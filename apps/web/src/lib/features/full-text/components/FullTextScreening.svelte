<script lang="ts">
	import {
		createGetProjectProtocol,
		createGetScreeningHistory,
		createScreenReport,
		createUndoScreening
	} from '$lib/api/generated/review/review';
	import {
		createGetReportDocument,
		createListDocumentPages,
		createListDocumentBlocks,
		createListDocumentReferences,
		createListDocumentSections,
		createListFullTextExclusionReasons,
		createListMissingFullText,
		createListReportDocuments,
		getListFullTextScreeningQueueQueryKey,
		getStreamReportDocumentContentUrl,
		listFullTextScreeningQueue
	} from '$lib/api/generated/documents/documents';
	import type {
		ApiErrorBody,
		DocumentBlockDto,
		DocumentPageDto,
		FullTextQueueItemDto,
		MissingFullTextDto,
		ScreeningDecisionInput,
		ScreeningStateDto
	} from '$lib/api/generated/models';
	import { page } from '$app/state';
	import AiProposalReview from '$lib/features/ai-assistance/components/AiProposalReview.svelte';
	import { canRequestAiSuggestions } from '$lib/features/ai-assistance/availability';
	import { createGetAiStatus } from '$lib/api/generated/ai/ai';
	import { resolve } from '$app/paths';
	import { pushState, replaceState } from '$app/navigation';
	import { ApiError } from '$lib/api/custom-fetch';
	import * as Alert from '@deepref/ui/alert';
	import { Button } from '@deepref/ui/button';
	import * as Resizable from '@deepref/ui/resizable';
	import PageTemplate from '$lib/shell/PageTemplate.svelte';
	import { Input } from '@deepref/ui/input';
	import { Skeleton } from '@deepref/ui/skeleton';
	import CriteriaPanel from '$lib/features/screening/components/CriteriaPanel.svelte';
	import ScreeningFeedback from '$lib/features/screening/components/ScreeningFeedback.svelte';
	import ScreeningHistory from '$lib/features/screening/components/ScreeningHistory.svelte';
	import { createInfiniteQuery, useQueryClient } from '@tanstack/svelte-query';
	import {
		attachExternalPdf,
		attachOpenAccessPdf,
		keepFlaggedPdf as keepDocumentIdentity,
		removeDocument,
		retryDocumentProcessing,
		uploadPdf
	} from '../api';
	import { describeDocumentFailure, isStoredPdfMissing } from '../document-failure';
	import { sortExclusionReasons } from '../reasons';
	import { applyFullTextState, type FullTextQueueCache } from '../optimistic';
	import CitedPassageBanner from './CitedPassageBanner.svelte';
	import DocumentStructurePanel from './DocumentStructurePanel.svelte';
	import EvidenceLink from './EvidenceLink.svelte';
	import FullTextDecisionBar from './FullTextDecisionBar.svelte';
	import PdfViewer from './PdfViewer.svelte';
	import ReparseDocument from './ReparseDocument.svelte';
	import { fullTextUrlString, parseFullTextUrl, type FullTextUrlState } from '../url';
	import {
		hasCommandModifier,
		hasOpenScreeningOverlay,
		isShortcutSuppressed,
		shortcutAction
	} from '$lib/features/screening/shortcuts';
	import { MediaQuery } from 'svelte/reactivity';
	import {
		ArrowLeft,
		ArrowRight,
		BookOpen,
		FileUp,
		FileWarning,
		Link2,
		RefreshCw,
		Trash2
	} from '@lucide/svelte';

	type FullTextItem = FullTextQueueItemDto | MissingFullTextDto;
	type AuthoritativeFullTextState = Pick<
		ScreeningStateDto,
		'report_id' | 'full_text_status' | 'full_text_exclusion_reason_id' | 'revision'
	>;
	type FullTextNavigationState = App.PageState & { deeprefFullTextSearch?: string };

	let { projectId }: { projectId: string } = $props();

	// Wait for status before showing suggestions, while failing open if status cannot load.
	const aiStatusQuery = createGetAiStatus();
	const aiSuggestionsAvailable = $derived(canRequestAiSuggestions(aiStatusQuery));
	const queryClient = useQueryClient();
	const fullTextQueueKey = $derived(
		getListFullTextScreeningQueueQueryKey(projectId, { limit: 100 })
	);
	const fullQueueQuery = createInfiniteQuery(() => ({
		queryKey: fullTextQueueKey,
		initialPageParam: undefined as string | undefined,
		queryFn: ({ pageParam, signal }) =>
			listFullTextScreeningQueue(projectId, { cursor: pageParam, limit: 100 }, { signal }),
		getNextPageParam: (lastPage) => lastPage.data.next_cursor ?? undefined,
		refetchInterval: 15_000
	}));
	const missingQuery = createListMissingFullText(
		() => projectId,
		() => ({ limit: 100 }),
		() => ({ query: { refetchInterval: 15_000 } })
	);
	const protocolQuery = createGetProjectProtocol(() => projectId);
	const reasonsQuery = createListFullTextExclusionReasons(() => projectId);
	const decisionMutation = createScreenReport();
	const undoMutation = createUndoScreening();

	const urlState = $derived.by(() => {
		const navigationSearch = (page.state as FullTextNavigationState).deeprefFullTextSearch;
		return parseFullTextUrl(new URLSearchParams(navigationSearch ?? page.url.search));
	});
	const queueItems = $derived(
		fullQueueQuery.data?.pages.flatMap((page) => page.data.items) ?? []
	);
	const missingItems = $derived(missingQuery.data?.data ?? []);
	const visibleQueueItems = $derived(
		queueItems.filter((item) => {
			if (urlState.filter === 'available') return item.document?.status === 'available';
			if (urlState.filter === 'failed') return item.document?.status === 'failed';
			return urlState.filter !== 'missing';
		})
	);
	const currentReportId = $derived(
		urlState.report ??
			(urlState.filter === 'missing'
				? missingItems[0]?.report_id
				: visibleQueueItems[0]?.report_id) ??
			null
	);
	const queueCurrent = $derived(
		visibleQueueItems.find((item) => item.report_id === currentReportId) ??
			(urlState.report
				? queueItems.find((item) => item.report_id === currentReportId)
				: undefined)
	);
	const missingCurrent = $derived(
		missingItems.find((item) => item.report_id === currentReportId)
	);
	const current = $derived<FullTextItem | null>(queueCurrent ?? missingCurrent ?? null);
	const protocol = $derived(protocolQuery.data?.data);
	const reasons = $derived(sortExclusionReasons(reasonsQuery.data?.data ?? []));
	const historyQuery = createGetScreeningHistory(
		() => projectId,
		() => currentReportId ?? '',
		() => ({ query: { enabled: Boolean(currentReportId), refetchInterval: 15_000 } })
	);
	const documentsQuery = createListReportDocuments(
		() => projectId,
		() => currentReportId ?? '',
		() => ({ limit: 10 }),
		() => ({ query: { enabled: Boolean(currentReportId), refetchInterval: 5_000 } })
	);
	const documentId = $derived(documentsQuery.data?.data[0]?.id ?? '');
	const selectedQueueDocumentId = $derived(queueCurrent?.document?.id ?? '');
	const effectiveDocumentId = $derived(selectedQueueDocumentId || documentId);
	const documentQuery = createGetReportDocument(
		() => projectId,
		() => currentReportId ?? '',
		() => effectiveDocumentId,
		() => ({
			query: {
				enabled: Boolean(currentReportId && effectiveDocumentId),
				refetchInterval: 5_000
			}
		})
	);
	const blocksQuery = createListDocumentBlocks(
		() => projectId,
		() => currentReportId ?? '',
		() => effectiveDocumentId,
		() => ({ limit: 100 }),
		() => ({ query: { enabled: Boolean(currentReportId && effectiveDocumentId) } })
	);
	const pagesQuery = createListDocumentPages(
		() => projectId,
		() => currentReportId ?? '',
		() => effectiveDocumentId,
		() => ({ query: { enabled: Boolean(currentReportId && effectiveDocumentId) } })
	);

	const sectionsQuery = createListDocumentSections(
		() => projectId,
		() => currentReportId ?? '',
		() => effectiveDocumentId,
		() => ({ query: { enabled: Boolean(currentReportId && effectiveDocumentId) } })
	);
	const referencesQuery = createListDocumentReferences(
		() => projectId,
		() => currentReportId ?? '',
		() => effectiveDocumentId,
		() => ({ query: { enabled: Boolean(currentReportId && effectiveDocumentId) } })
	);

	let choosingReason = $state(false);
	let documentView = $state<'pdf' | 'outline' | 'references' | 'blocks'>('pdf');
	let statusMessage = $state('');
	let errorMessage = $state('');
	let externalUrl = $state('');
	let uploading = $state(false);
	let retrying = $state(false);
	let finding = $state(false);
	let keeping = $state(false);
	let removing = $state(false);

	// A confirmation belongs to the report it was recorded for.
	$effect(() => {
		void currentReportId;
		statusMessage = '';
	});

	const historyItems = $derived(historyQuery.data?.data.items ?? []);
	const latestHistory = $derived(historyItems.at(-1) ?? null);
	const canUndo = $derived(
		Boolean(latestHistory?.stage === 'full_text' && latestHistory.event_kind === 'decision')
	);
	const document = $derived(documentQuery.data?.data);
	const blocks = $derived(blocksQuery.data?.data ?? []);
	const pages = $derived<DocumentPageDto[]>(pagesQuery.data?.data ?? []);
	const selectedBlockId = $derived(urlState.block);
	const selectedBlock = $derived(blocks.find((block) => block.id === selectedBlockId));
	const sections = $derived(sectionsQuery.data?.data ?? []);
	const references = $derived(referencesQuery.data?.data ?? []);
	// The stored file is gone: the parsed text survives, the page view cannot load.
	const storedPdfMissing = $derived(
		document?.status === 'available' && isStoredPdfMissing(document.parser_error)
	);
	// A PDF whose title or DOI disagrees with the report: a warning until the researcher decides.
	const identityFlag = $derived(
		document?.identity?.verdict === 'mismatch' && !document.identity.acknowledged_at
			? document.identity
			: null
	);
	const identityHeadline = $derived(
		identityFlag?.detected_title
			? `This PDF may belong to another article: ${identityFlag.detected_title}`
			: identityFlag?.detected_doi
				? `This PDF may belong to another article (DOI ${identityFlag.detected_doi})`
				: 'This PDF may belong to another article'
	);
	const currentDoi = $derived(current?.doi ?? null);
	const screenStatus = $derived(queueCurrent?.full_text_status ?? 'unscreened');
	const screenRevision = $derived(queueCurrent?.revision ?? 0);
	const usableFullText = $derived(
		document?.status === 'available' && Boolean(document.parser_version)
	);
	const contentUrl = $derived(
		effectiveDocumentId && currentReportId
			? getStreamReportDocumentContentUrl(projectId, currentReportId, effectiveDocumentId)
			: ''
	);
	const currentIndex = $derived(
		(urlState.filter === 'missing' ? missingItems : visibleQueueItems).findIndex(
			(item) => item.report_id === currentReportId
		)
	);
	const queueCount = $derived(
		urlState.filter === 'missing' ? missingItems.length : visibleQueueItems.length
	);

	const wide = new MediaQuery('(min-width: 1024px)');
	const listedItems = $derived<FullTextItem[]>(
		urlState.filter === 'missing' ? missingItems : visibleQueueItems
	);
	const readyCount = $derived(
		queueItems.filter((item) => item.document?.status === 'available').length
	);
	const filterOptions = $derived<
		{ value: FullTextUrlState['filter']; label: string; count: number }[]
	>([
		{ value: 'all', label: 'All', count: queueItems.length },
		{ value: 'available', label: 'PDF ready', count: readyCount },
		{ value: 'missing', label: 'Needs PDF', count: missingItems.length }
	]);

	function statusLabel(value: string) {
		return value.replaceAll('_', ' ');
	}

	type DocumentState = 'ready' | 'parsing' | 'failed' | 'missing';

	function documentState(item: FullTextItem): DocumentState {
		if ('document' in item) {
			const status = item.document?.status;
			if (!status) return 'missing';
			if (status === 'available') return 'ready';
			return status === 'failed' ? 'failed' : 'parsing';
		}
		return 'status' in item && item.status === 'failed' ? 'failed' : 'missing';
	}

	const documentStateLabel: Record<DocumentState, string> = {
		ready: 'PDF ready',
		parsing: 'Parsing',
		failed: 'PDF failed · retry available',
		missing: 'No PDF'
	};

	/** A link that cannot yield a PDF is not "retry available": say so in the queue. */
	function queueLabel(item: FullTextItem, state: DocumentState): string {
		if (state === 'failed' && 'document' in item && item.document?.parser_error) {
			if (!describeDocumentFailure(item.document.parser_error).retryable) {
				return 'PDF link failed';
			}
		}
		return documentStateLabel[state];
	}

	function decisionOf(item: FullTextItem): 'include' | 'exclude' | 'maybe' | undefined {
		const status = 'full_text_status' in item ? item.full_text_status : undefined;
		return status === 'include' || status === 'exclude' || status === 'maybe'
			? status
			: undefined;
	}

	const decisionLabel = { include: 'Included', exclude: 'Excluded', maybe: 'Maybe' } as const;

	function handleKeydown(event: KeyboardEvent) {
		if (
			event.defaultPrevented ||
			hasCommandModifier(event) ||
			isShortcutSuppressed(event.target, hasOpenScreeningOverlay())
		)
			return;
		if (choosingReason) {
			const index = Number(event.key) - 1;
			const options = reasons.filter((reason) => reason.stage === 'full_text');
			if (Number.isInteger(index) && options[index]) {
				event.preventDefault();
				choosingReason = false;
				void decide('exclude', options[index].id);
			} else if (event.key === 'Escape') {
				event.preventDefault();
				choosingReason = false;
			}
			return;
		}
		const action = shortcutAction(event.key);
		if (!action) return;
		event.preventDefault();
		if (action === 'previous' || action === 'next') return void move(action);
		if (action === 'undo') return void undo();
		if (!usableFullText) return;
		if (action === 'exclude') choosingReason = true;
		else void decide(action, null);
	}

	function updateUrl(changes: Partial<FullTextUrlState>, replace = true) {
		const next = { ...urlState, ...changes };
		const destination = (resolve('/projects/[projectId]/screening/full-text', { projectId }) +
			fullTextUrlString(next)) as
			| `/projects/${string}/screening/full-text`
			| `/projects/${string}/screening/full-text?${string}`;
		const navigationState: FullTextNavigationState = {
			...page.state,
			deeprefFullTextSearch: fullTextUrlString(next)
		};
		if (replace) {
			replaceState(resolve(destination), navigationState);
		} else {
			pushState(resolve(destination), navigationState);
		}
	}

	function isAuthoritativeState(value: unknown): value is AuthoritativeFullTextState {
		if (!value || typeof value !== 'object') return false;
		if (!('report_id' in value) || !('full_text_status' in value) || !('revision' in value))
			return false;
		const state = value as Record<string, unknown>;
		return (
			typeof state.report_id === 'string' &&
			typeof state.full_text_status === 'string' &&
			Number.isSafeInteger(state.revision) &&
			(state.full_text_exclusion_reason_id === null ||
				typeof state.full_text_exclusion_reason_id === 'string')
		);
	}

	function currentStateFromError(error: unknown): AuthoritativeFullTextState | null {
		if (!(error instanceof ApiError)) return null;
		const info = error.info as ApiErrorBody | null;
		const details = info?.details;
		if (!details || typeof details !== 'object' || !('currentState' in details)) return null;
		const state = details.currentState;
		return isAuthoritativeState(state) && state.report_id === currentReportId ? state : null;
	}

	function acceptAuthoritativeState(state: AuthoritativeFullTextState, message: string) {
		queryClient.setQueryData<FullTextQueueCache>(fullTextQueueKey, (cache) =>
			applyFullTextState(cache, state)
		);
		statusMessage = message;
	}

	function selectReport(reportId: string, push = true) {
		return updateUrl({ report: reportId, page: null, block: null }, !push);
	}

	async function move(direction: 'previous' | 'next') {
		const items = urlState.filter === 'missing' ? missingItems : visibleQueueItems;
		const index = items.findIndex((item) => item.report_id === currentReportId);
		const next = items[index + (direction === 'next' ? 1 : -1)];
		if (next) await selectReport(next.report_id);
	}

	async function loadMoreReports() {
		if (!fullQueueQuery.hasNextPage || fullQueueQuery.isFetchingNextPage) return;
		await fullQueueQuery.fetchNextPage();
	}

	function handleBlockSelect(block: DocumentBlockDto) {
		void updateUrl(
			{ report: currentReportId, page: block.page_number, block: block.id },
			false
		);
	}

	function handleSectionSelect(block: DocumentBlockDto) {
		documentView = 'pdf';
		handleBlockSelect(block);
	}

	function refreshStructure() {
		void Promise.all([
			documentQuery.refetch(),
			blocksQuery.refetch(),
			pagesQuery.refetch(),
			sectionsQuery.refetch(),
			referencesQuery.refetch()
		]);
	}

	function handleAiEvidenceSelect(evidence: {
		document_block_id: string;
		page: number;
		section_path: string[];
	}) {
		void updateUrl(
			{ report: currentReportId, page: evidence.page, block: evidence.document_block_id },
			false
		);
	}

	async function decide(decision: ScreeningDecisionInput, reasonId: string | null) {
		if (
			!currentReportId ||
			!protocol ||
			!usableFullText ||
			decisionMutation.isPending ||
			undoMutation.isPending
		)
			return;
		if (decision === 'exclude' && !reasonId) {
			errorMessage = 'Choose exactly one full-text exclusion reason.';
			return;
		}
		const previous = queryClient.getQueryData<FullTextQueueCache>(fullTextQueueKey);
		const expectedRevision = screenRevision;
		const nextState: AuthoritativeFullTextState = {
			report_id: currentReportId,
			full_text_status: decision,
			full_text_exclusion_reason_id: decision === 'exclude' ? reasonId : null,
			revision: expectedRevision + 1
		};
		queryClient.setQueryData<FullTextQueueCache>(fullTextQueueKey, (cache) =>
			applyFullTextState(cache, nextState)
		);
		errorMessage = '';
		try {
			const data = {
				stage: 'full_text' as const,
				decision,
				protocol_version_id: protocol.id,
				expected_revision: expectedRevision,
				...(decision === 'exclude' ? { exclusion_reason_id: reasonId } : {})
			};
			const result = await decisionMutation.mutateAsync({
				projectId,
				reportId: currentReportId,
				data
			});
			acceptAuthoritativeState(result.data, `Full-text ${decision} recorded.`);
			await queryClient.invalidateQueries({ queryKey: fullTextQueueKey });
			await queryClient.invalidateQueries({ queryKey: missingQuery.queryKey });
		} catch (error) {
			const state = currentStateFromError(error);
			if (state) {
				queryClient.setQueryData<FullTextQueueCache>(fullTextQueueKey, (cache) =>
					applyFullTextState(cache, state)
				);
				statusMessage =
					'This report changed elsewhere. The authoritative state is loaded for review.';
				await historyQuery.refetch();
			} else {
				queryClient.setQueryData(fullTextQueueKey, previous);
				errorMessage =
					error instanceof Error ? error.message : 'The full-text decision failed.';
			}
		}
	}

	async function undo() {
		if (
			!currentReportId ||
			!protocol ||
			!canUndo ||
			undoMutation.isPending ||
			decisionMutation.isPending
		)
			return;
		const previous = queryClient.getQueryData<FullTextQueueCache>(fullTextQueueKey);
		queryClient.setQueryData<FullTextQueueCache>(fullTextQueueKey, (cache) =>
			applyFullTextState(cache, {
				report_id: currentReportId,
				full_text_status: latestHistory?.previous_full_text_status ?? 'unscreened',
				revision: screenRevision + 1
			})
		);
		try {
			const result = await undoMutation.mutateAsync({
				projectId,
				reportId: currentReportId,
				data: {
					stage: 'full_text',
					protocol_version_id: protocol.id,
					expected_revision: screenRevision
				}
			});
			acceptAuthoritativeState(result.data, 'The last full-text decision was undone.');
			await historyQuery.refetch();
		} catch (error) {
			const state = currentStateFromError(error);
			if (state) {
				queryClient.setQueryData<FullTextQueueCache>(fullTextQueueKey, (cache) =>
					applyFullTextState(cache, state)
				);
			} else {
				queryClient.setQueryData(fullTextQueueKey, previous);
			}
			errorMessage = state
				? 'The report changed elsewhere; review the authoritative state.'
				: error instanceof Error
					? error.message
					: 'Undo failed.';
		}
	}

	function upload(event: Event) {
		if (!(event.currentTarget instanceof HTMLInputElement)) return;
		const file = event.currentTarget.files?.[0];
		if (!file || !currentReportId) return;
		errorMessage = '';
		uploading = true;
		void uploadPdf(projectId, currentReportId, file)
			.then(async () => {
				statusMessage = 'PDF uploaded. Parsing status will refresh automatically.';
				await Promise.all([
					documentsQuery.refetch(),
					queryClient.invalidateQueries({ queryKey: fullTextQueueKey }),
					queryClient.invalidateQueries({ queryKey: missingQuery.queryKey })
				]);
			})
			.catch((error: unknown) => {
				errorMessage = error instanceof Error ? error.message : 'Upload failed.';
			})
			.finally(() => {
				uploading = false;
				if (event.currentTarget instanceof HTMLInputElement) event.currentTarget.value = '';
			});
	}

	function retry() {
		if (!currentReportId || !document || retrying) return;
		errorMessage = '';
		retrying = true;
		void retryDocumentProcessing(projectId, currentReportId, document.id)
			.then(async () => {
				statusMessage = 'Processing restarted. The PDF will open when it is ready.';
				await Promise.all([
					documentQuery.refetch(),
					documentsQuery.refetch(),
					queryClient.invalidateQueries({ queryKey: fullTextQueueKey }),
					queryClient.invalidateQueries({ queryKey: missingQuery.queryKey })
				]);
			})
			.catch((error: unknown) => {
				errorMessage = error instanceof Error ? error.message : 'Retry failed.';
			})
			.finally(() => {
				retrying = false;
			});
	}

	function attachExternal() {
		if (!currentReportId || !externalUrl.trim()) return;
		uploading = true;
		void attachExternalPdf(projectId, currentReportId, {
			url: externalUrl.trim(),
			original_filename: null
		})
			.then(async () => {
				statusMessage = 'External PDF queued for guarded retrieval and parsing.';
				externalUrl = '';
				await Promise.all([
					documentsQuery.refetch(),
					queryClient.invalidateQueries({ queryKey: fullTextQueueKey }),
					queryClient.invalidateQueries({ queryKey: missingQuery.queryKey })
				]);
			})
			.catch((error: unknown) => {
				errorMessage =
					error instanceof Error ? error.message : 'External attachment failed.';
			})
			.finally(() => {
				uploading = false;
			});
	}
	function findOpenAccess() {
		if (!currentReportId || finding || uploading) return;
		errorMessage = '';
		finding = true;
		void attachOpenAccessPdf(projectId, currentReportId)
			.then(async () => {
				statusMessage = 'Open-access PDF found and queued for retrieval and parsing.';
				await Promise.all([
					documentsQuery.refetch(),
					queryClient.invalidateQueries({ queryKey: fullTextQueueKey }),
					queryClient.invalidateQueries({ queryKey: missingQuery.queryKey })
				]);
			})
			.catch((error: unknown) => {
				errorMessage =
					error instanceof Error ? error.message : 'The open-access lookup failed.';
			})
			.finally(() => {
				finding = false;
			});
	}

	function keepFlaggedPdf() {
		if (!currentReportId || !document || keeping || removing) return;
		errorMessage = '';
		keeping = true;
		void keepDocumentIdentity(projectId, currentReportId, document.id)
			.then(async () => {
				statusMessage = 'Kept. The PDF stays attached to this report.';
				await Promise.all([documentQuery.refetch(), documentsQuery.refetch()]);
			})
			.catch((error: unknown) => {
				errorMessage = error instanceof Error ? error.message : 'Keeping the PDF failed.';
			})
			.finally(() => {
				keeping = false;
			});
	}

	function removeFlaggedPdf() {
		if (!currentReportId || !document || keeping || removing) return;
		errorMessage = '';
		removing = true;
		void removeDocument(projectId, currentReportId, document.id)
			.then(async () => {
				statusMessage = 'PDF removed. Upload or link the right article.';
				await Promise.all([
					documentsQuery.refetch(),
					queryClient.invalidateQueries({ queryKey: fullTextQueueKey }),
					queryClient.invalidateQueries({ queryKey: missingQuery.queryKey })
				]);
			})
			.catch((error: unknown) => {
				errorMessage = error instanceof Error ? error.message : 'Removing the PDF failed.';
			})
			.finally(() => {
				removing = false;
			});
	}
</script>

<svelte:window onkeydown={handleKeydown} />

{#snippet queuePane()}
	<div class="flex flex-wrap gap-1" role="group" aria-label="Full-text queue filters">
		{#each filterOptions as option (option.value)}
			<button
				type="button"
				class={[
					'inline-flex h-7 shrink-0 items-center gap-1.5 rounded-md px-2 text-xs font-medium whitespace-nowrap transition-colors focus-visible:outline-2 focus-visible:outline-ring',
					urlState.filter === option.value
						? 'bg-primary text-primary-foreground'
						: 'text-muted-foreground hover:bg-muted hover:text-foreground'
				]}
				aria-pressed={urlState.filter === option.value}
				onclick={() => void updateUrl({ filter: option.value, report: null })}
				>{option.label}<span class="tabular-nums opacity-70">{option.count}</span></button
			>
		{/each}
	</div>
{/snippet}

{#snippet queueList()}
	{#if fullQueueQuery.isPending || missingQuery.isPending}
		<div class="flex flex-col gap-2 p-2">
			{#each { length: 5 }, index (index)}<Skeleton class="h-12 w-full" />{/each}
		</div>
	{:else if listedItems.length === 0}
		<p class="p-3 text-xs text-muted-foreground">
			{urlState.filter === 'missing'
				? 'Every included report has a PDF.'
				: 'No included reports in this view.'}
		</p>
	{:else}
		<ol class="flex flex-col p-1.5" aria-label="Full-text queue">
			{#each listedItems as item (item.report_id)}
				{@const selected = item.report_id === currentReportId}
				{@const state = documentState(item)}
				{@const label = queueLabel(item, state)}
				{@const decision = decisionOf(item)}
				<li>
					<button
						type="button"
						class={[
							'flex w-full flex-col items-start gap-1 rounded-md px-2.5 py-2 text-left text-sm transition-colors hover:bg-muted focus-visible:outline-2 focus-visible:outline-ring',
							selected && 'bg-accent shadow-inset-accent'
						]}
						aria-current={selected ? 'true' : undefined}
						onclick={() => void selectReport(item.report_id)}
					>
						<span class={['line-clamp-2 leading-snug', selected && 'font-medium']}
							>{item.title ?? 'Untitled'}</span
						>
						<span class="flex items-center gap-2 text-xs text-muted-foreground">
							<span class={[state !== 'ready' && 'text-warning']}>{label}</span>
							{#if decision}
								<span class="flex items-center gap-1">
									<span
										class={[
											'size-1.5 rounded-full',
											decision === 'include' && 'bg-success',
											decision === 'exclude' && 'bg-destructive',
											decision === 'maybe' && 'bg-warning'
										]}
										aria-hidden="true"
									></span>{decisionLabel[decision]}
								</span>
							{/if}
						</span>
					</button>
				</li>
			{/each}
			{#if urlState.filter !== 'missing' && fullQueueQuery.hasNextPage}
				<li class="p-1">
					<Button
						variant="ghost"
						size="sm"
						class="w-full"
						disabled={fullQueueQuery.isFetchingNextPage}
						onclick={() => void loadMoreReports()}
						>{fullQueueQuery.isFetchingNextPage
							? 'Loading more…'
							: 'Load more reports'}</Button
					>
				</li>
			{/if}
		</ol>
	{/if}
{/snippet}

{#snippet attachPdf()}
	<div class="flex max-w-lg flex-col gap-4" data-testid="full-text-attach">
		<div class="flex flex-col gap-1">
			<p class="font-medium">
				{document?.status === 'failed'
					? isStoredPdfMissing(document.parser_error)
						? 'The stored PDF file is missing'
						: 'The attached PDF could not be processed'
					: document?.status
						? `PDF ${statusLabel(document.status)}…`
						: 'No PDF attached'}
			</p>
			{#if document?.status === 'failed'}
				{@const failure = describeDocumentFailure(document.parser_error)}
				{@const missing = isStoredPdfMissing(document.parser_error)}
				<p class="text-sm text-muted-foreground" data-testid="full-text-failure">
					{failure.summary}
					<span class="block">
						{missing
							? 'Upload the PDF again to parse it.'
							: failure.retryable
								? 'Try again, or upload a different PDF.'
								: 'Paste a different link, or upload the PDF.'}
					</span>
				</p>
				{#if failure.detail}
					<details class="text-xs text-muted-foreground">
						<summary class="cursor-pointer hover:text-foreground"
							>Technical detail</summary
						>
						<p class="pt-1 break-words">{failure.detail}</p>
					</details>
				{/if}
			{:else}
				<p class="text-sm text-muted-foreground">
					{document?.status
						? 'The viewer opens when parsing finishes. You can still replace the file.'
						: 'Upload the full text or paste a link to it. Decisions unlock once it is parsed.'}
				</p>
			{/if}
		</div>
		{#if document?.status === 'failed' && !isStoredPdfMissing(document.parser_error) && describeDocumentFailure(document.parser_error).retryable}
			<div class="flex flex-wrap gap-2">
				<Button type="button" disabled={retrying || uploading} onclick={retry}
					><RefreshCw data-icon="inline-start" />{retrying
						? 'Retrying…'
						: 'Retry processing'}</Button
				>
			</div>
		{/if}
		<label class="inline-flex cursor-pointer items-center self-start"
			><span class="sr-only">Upload PDF</span><input
				class="hidden"
				type="file"
				accept="application/pdf,.pdf"
				onchange={upload}
				disabled={uploading}
			/><Button
				type="button"
				variant={document?.status === 'failed' ? 'outline' : 'default'}
				disabled={uploading}
				onclick={(event) => {
					const target = event.currentTarget.parentElement?.querySelector('input');
					if (target instanceof HTMLInputElement) target.click();
				}}
				><FileUp data-icon="inline-start" />{uploading
					? 'Uploading…'
					: document?.status === 'failed'
						? 'Upload a different PDF'
						: 'Upload PDF'}</Button
			></label
		>
		{#if currentDoi}
			<div class="flex flex-col gap-1.5">
				<Button
					type="button"
					variant="outline"
					class="self-start"
					disabled={finding || uploading}
					onclick={findOpenAccess}
					><BookOpen data-icon="inline-start" />{finding
						? 'Looking up…'
						: 'Find open-access PDF'}</Button
				>
				<p class="text-xs text-muted-foreground">
					Looks up a free copy by this report's DOI, through Unpaywall.
				</p>
			</div>
		{/if}
		<form
			class="flex gap-2"
			onsubmit={(event) => {
				event.preventDefault();
				attachExternal();
			}}
		>
			<Input
				bind:value={externalUrl}
				type="url"
				placeholder="or paste a PDF link"
				aria-label="External PDF URL"
			/>
			<Button type="submit" variant="outline" disabled={uploading || !externalUrl.trim()}
				><Link2 data-icon="inline-start" />Attach</Button
			>
		</form>
	</div>
{/snippet}

{#snippet documentSurface()}
	{#if identityFlag}
		<Alert.Root variant="warning" class="mx-6 mt-6 w-auto" data-testid="identity-warning">
			<Alert.Title>{identityHeadline}</Alert.Title>
			<Alert.Description>
				<p>
					The title or DOI on this PDF does not match the report. Keep it if it is the
					right article, or remove it and attach the right one.
				</p>
				<div class="mt-2 flex flex-wrap gap-2">
					<Button
						type="button"
						variant="secondary"
						size="sm"
						disabled={keeping || removing}
						onclick={keepFlaggedPdf}>{keeping ? 'Keeping…' : 'Keep anyway'}</Button
					>
					<Button
						type="button"
						variant="outline"
						size="sm"
						disabled={keeping || removing}
						onclick={removeFlaggedPdf}
						><Trash2 data-icon="inline-start" />{removing
							? 'Removing…'
							: 'Remove'}</Button
					>
				</div>
			</Alert.Description>
		</Alert.Root>
	{/if}
	{#if documentView === 'blocks'}
		<div class="flex flex-col gap-3 p-6">
			<p class="text-xs text-muted-foreground">
				Parser {document?.parser_version ?? 'pending'}{document?.ocr_required
					? ' · OCR required on one or more pages'
					: ''}
			</p>
			{#if blocks.length === 0}
				<p class="text-sm text-muted-foreground">
					Evidence blocks appear after the PDF is parsed.
				</p>
			{:else}
				<ol class="flex flex-col gap-2" aria-label="Parsed evidence blocks">
					{#each blocks as block (block.id)}<li>
							<EvidenceLink
								{block}
								selected={selectedBlockId === block.id}
								onSelect={(value) => {
									documentView = 'pdf';
									handleBlockSelect(value);
								}}
							/>
						</li>{/each}
				</ol>
			{/if}
		</div>
	{:else if documentView === 'outline' || documentView === 'references'}
		<DocumentStructurePanel
			view={documentView}
			{sections}
			{references}
			{blocks}
			loading={documentView === 'outline'
				? sectionsQuery.isLoading
				: referencesQuery.isLoading}
			errorMessage={(documentView === 'outline' ? sectionsQuery.error : referencesQuery.error)
				?.message ?? ''}
			onSelectSection={handleSectionSelect}
		/>
	{:else if storedPdfMissing}
		<div class="flex flex-col gap-3 p-6" data-testid="stored-pdf-missing">
			<Alert.Root>
				<FileWarning aria-hidden="true" />
				<Alert.Title>The stored PDF file is missing</Alert.Title>
				<Alert.Description>
					The parsed text is still available under Parsed evidence. Upload the PDF again
					from Replace PDF to restore the page view and to re-parse it.
				</Alert.Description>
			</Alert.Root>
		</div>
	{:else if contentUrl && document?.status === 'available'}
		<PdfViewer
			fill={wide.current}
			{contentUrl}
			{blocks}
			pageMetadata={pages}
			selectedPage={urlState.page}
			{selectedBlockId}
			onBlockSelect={handleBlockSelect}
		/>
	{:else}
		<div class="p-6">{@render attachPdf()}</div>
	{/if}
{/snippet}

{#snippet header()}
	{#if current}
		<div class="flex flex-col gap-1">
			<div class="flex items-start justify-between gap-3">
				<h2 class="editorial-title text-xl leading-tight">
					{current.title ?? 'Untitled report'}
				</h2>
				<div class="flex shrink-0 items-center gap-1">
					<span class="mr-1 text-xs text-muted-foreground tabular-nums"
						>{currentIndex >= 0
							? `${currentIndex + 1} of ${queueCount}`
							: urlState.filter === 'missing' && queueCurrent
								? 'Attached · left this view'
								: ''}</span
					>
					<Button
						variant="ghost"
						size="icon-sm"
						aria-label="Previous report (ArrowLeft)"
						onclick={() => void move('previous')}
						><ArrowLeft aria-hidden="true" /></Button
					>
					<Button
						variant="ghost"
						size="icon-sm"
						aria-label="Next report (ArrowRight)"
						onclick={() => void move('next')}><ArrowRight aria-hidden="true" /></Button
					>
				</div>
			</div>
			<div class="flex flex-wrap items-center gap-x-4 gap-y-1 text-xs text-muted-foreground">
				<details class="group">
					<summary class="cursor-pointer hover:text-foreground">Abstract</summary>
					<p class="max-w-3xl pt-2 text-sm leading-relaxed text-foreground/90">
						{current.abstract_text ?? 'No abstract is available.'}
					</p>
				</details>
				{#if document?.status === 'available'}
					<div
						class="flex items-center gap-1 whitespace-nowrap"
						role="group"
						aria-label="Document view"
					>
						<button
							type="button"
							class={[
								'hover:text-foreground',
								documentView === 'pdf' && 'text-foreground'
							]}
							aria-pressed={documentView === 'pdf'}
							onclick={() => (documentView = 'pdf')}>PDF</button
						>
						<span aria-hidden="true">·</span>
						<button
							type="button"
							class={[
								'hover:text-foreground',
								documentView === 'outline' && 'text-foreground'
							]}
							aria-pressed={documentView === 'outline'}
							onclick={() => (documentView = 'outline')}
							>Outline{sectionsQuery.data ? ` (${sections.length})` : ''}</button
						>
						<span aria-hidden="true">·</span>
						<button
							type="button"
							class={[
								'hover:text-foreground',
								documentView === 'references' && 'text-foreground'
							]}
							aria-pressed={documentView === 'references'}
							onclick={() => (documentView = 'references')}
							>References{referencesQuery.data
								? ` (${references.length})`
								: ''}</button
						>
						<span aria-hidden="true">·</span>
						<button
							type="button"
							class={[
								'hover:text-foreground',
								documentView === 'blocks' && 'text-foreground'
							]}
							aria-pressed={documentView === 'blocks'}
							onclick={() => (documentView = 'blocks')}
							>Parsed evidence · {blocks.length} blocks</button
						>
					</div>
					<details>
						<summary class="cursor-pointer hover:text-foreground">Replace PDF</summary>
						<div class="flex flex-col gap-6 pt-3">
							{@render attachPdf()}
							<ReparseDocument
								{projectId}
								reportId={currentReportId ?? ''}
								documentId={effectiveDocumentId}
								{document}
								onFinished={refreshStructure}
							/>
						</div>
					</details>
				{/if}
			</div>
			{#if selectedBlockId && !selectedBlock && blocks.length > 0}
				<p
					class="rounded-md border border-warning/40 bg-warning/10 p-2 text-xs text-muted-foreground"
					role="status"
					data-testid="cited-passage-missing"
				>
					This citation points to a passage that is not in the PDF attached to this report
					now. It may come from a PDF that was replaced.
				</p>
			{/if}
			{#if selectedBlock}
				<CitedPassageBanner
					block={selectedBlock}
					onClear={() => void updateUrl({ page: null, block: null })}
				/>
			{/if}
		</div>
	{/if}
{/snippet}

{#snippet decisionBar()}
	<FullTextDecisionBar
		reasons={reasons.filter((reason) => reason.stage === 'full_text')}
		current={screenStatus}
		bind:choosingReason
		pending={decisionMutation.isPending || undoMutation.isPending}
		{canUndo}
		{statusMessage}
		available={usableFullText}
		onDecision={(decision, reasonId) => void decide(decision, reasonId)}
		onUndo={() => void undo()}
	/>
{/snippet}

{#snippet context()}
	<CriteriaPanel
		criteria={protocol?.criteria ?? []}
		protocolVersion={protocol?.version}
		stage="full_text"
	/>
	{#if aiSuggestionsAvailable}
		<details class="disclosure">
			<summary>Get an AI suggestion</summary>
			{#if currentReportId}<AiProposalReview
					{projectId}
					reportId={currentReportId}
					stage="full_text"
					protocolVersionId={protocol?.id}
					expectedRevision={screenRevision}
					onEvidenceSelect={handleAiEvidenceSelect}
				/>{/if}
		</details>
	{/if}
	{#if currentReportId}<ScreeningHistory items={historyItems} />{/if}
{/snippet}

{#snippet emptyState()}
	{#if fullQueueQuery.isPending || missingQuery.isPending}
		<div class="flex flex-col gap-3 p-6" aria-live="polite">
			<Skeleton class="h-6 w-2/3" /><Skeleton class="h-32 w-full" />
			<p class="text-sm text-muted-foreground">Loading full-text queue…</p>
		</div>
	{:else}
		<div class="flex flex-col items-start gap-3 p-6">
			{#if queueItems.length === 0}
				<h2 class="editorial-title text-xl">No reports have reached full text</h2>
				<p class="text-sm text-muted-foreground">
					Reports included at title & abstract appear here for full-text assessment.
				</p>
				<Button
					variant="outline"
					href={resolve('/projects/[projectId]/screening/title-abstract', { projectId })}
					>Open title & abstract</Button
				>
			{:else if urlState.filter === 'available'}
				<h2 class="editorial-title text-xl">No PDFs are ready yet</h2>
				<p class="text-sm text-muted-foreground">
					{missingItems.length} included {missingItems.length === 1
						? 'report needs'
						: 'reports need'} a PDF before full-text assessment.
				</p>
				<Button onclick={() => void updateUrl({ filter: 'missing', report: null })}
					>Attach missing PDFs</Button
				>
			{:else}
				<h2 class="editorial-title text-xl">Every included report has a PDF</h2>
				<Button
					variant="outline"
					onclick={() => void updateUrl({ filter: 'available', report: null })}
					>Assess reports with PDFs</Button
				>
			{/if}
		</div>
	{/if}
{/snippet}

{#if wide.current}
	<div class="flex min-h-0 flex-1 flex-col" data-testid="full-text-page">
		<Resizable.PaneGroup
			direction="horizontal"
			class="min-h-0 flex-1"
			autoSaveId="deepref:full-text-layout"
		>
			<Resizable.Pane order={1} defaultSize={20} minSize={14} maxSize={34}>
				<aside class="flex h-full min-h-0 flex-col" aria-label="Full-text queue list">
					<div class="border-b p-3">{@render queuePane()}</div>
					<div class="min-h-0 flex-1 overflow-y-auto">{@render queueList()}</div>
				</aside>
			</Resizable.Pane>
			<Resizable.Handle />
			<Resizable.Pane order={2} defaultSize={56} minSize={36}>
				<section class="flex h-full min-h-0 flex-col" aria-label="Full-text review">
					{#if current}
						<div class="border-b px-6 py-3">{@render header()}</div>
						<div class="relative min-h-0 flex-1">
							<div class="h-full overflow-y-auto">{@render documentSurface()}</div>
							{#if errorMessage}
								<div class="absolute inset-x-4 top-3 z-10 shadow-md">
									<ScreeningFeedback
										errorTitle="Full-text review needs attention"
										{errorMessage}
									/>
								</div>
							{/if}
						</div>
						<div class="border-t px-6 py-3">{@render decisionBar()}</div>
					{:else}
						{@render emptyState()}
					{/if}
				</section>
			</Resizable.Pane>
			<Resizable.Handle />
			<Resizable.Pane order={3} defaultSize={24} minSize={16} maxSize={36}>
				<aside
					class="flex h-full min-h-0 flex-col gap-6 overflow-y-auto px-5 py-5"
					aria-label="Criteria and history"
				>
					{@render context()}
				</aside>
			</Resizable.Pane>
		</Resizable.PaneGroup>
	</div>
{:else}
	<PageTemplate testId="full-text-page" containerClass="gap-4">
		{@render queuePane()}
		<ScreeningFeedback errorTitle="Full-text review needs attention" {errorMessage} />
		<section class="flex flex-col gap-4" aria-label="Full-text review">
			{#if current}
				{@render header()}
				<div class="-mx-4 border-y">{@render documentSurface()}</div>
			{:else}
				{@render emptyState()}
			{/if}
		</section>
		{#if current}
			<div class="sticky bottom-0 -mx-4 border-t bg-background px-4 py-3">
				{@render decisionBar()}
			</div>
		{/if}
		<details class="disclosure">
			<summary>Eligibility criteria</summary>
			<div class="flex flex-col gap-6 pt-2">{@render context()}</div>
		</details>
	</PageTemplate>
{/if}
