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
	import { resolve } from '$app/paths';
	import { pushState, replaceState } from '$app/navigation';
	import { ApiError } from '$lib/api/custom-fetch';
	import * as Alert from '$lib/components/ui/alert';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';
	import { Input } from '$lib/components/ui/input';
	import CriteriaPanel from '$lib/features/screening/components/CriteriaPanel.svelte';
	import ScreeningHistory from '$lib/features/screening/components/ScreeningHistory.svelte';
	import { createInfiniteQuery, useQueryClient } from '@tanstack/svelte-query';
	import { attachExternalPdf, uploadPdf } from '../api';
	import { applyFullTextState, type FullTextQueueCache } from '../optimistic';
	import EvidenceLink from './EvidenceLink.svelte';
	import FullTextDecisionBar from './FullTextDecisionBar.svelte';
	import PdfViewer from './PdfViewer.svelte';
	import { fullTextUrlString, parseFullTextUrl, type FullTextUrlState } from '../url';

	type FullTextItem = FullTextQueueItemDto | MissingFullTextDto;
	type AuthoritativeFullTextState = Pick<
		ScreeningStateDto,
		'report_id' | 'full_text_status' | 'full_text_exclusion_reason_id' | 'revision'
	>;
	type FullTextNavigationState = App.PageState & { deeprefFullTextSearch?: string };

	let { projectId }: { projectId: string } = $props();
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
	const reasons = $derived(reasonsQuery.data?.data ?? []);
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

	let selectedReason = $state('');
	let statusMessage = $state('');
	let errorMessage = $state('');
	let externalUrl = $state('');
	let externalFilename = $state('');
	let uploading = $state(false);

	const historyItems = $derived(historyQuery.data?.data.items ?? []);
	const latestHistory = $derived(historyItems.at(-1) ?? null);
	const canUndo = $derived(
		Boolean(latestHistory?.stage === 'full_text' && latestHistory.event_kind === 'decision')
	);
	const document = $derived(documentQuery.data?.data);
	const blocks = $derived(blocksQuery.data?.data ?? []);
	const pages = $derived<DocumentPageDto[]>(pagesQuery.data?.data ?? []);
	const selectedBlockId = $derived(urlState.block);
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
			const authoritative = {
				report_id: result.data.report_id,
				full_text_status: result.data.full_text_status,
				full_text_exclusion_reason_id: result.data.full_text_exclusion_reason_id,
				revision: result.data.revision
			};
			queryClient.setQueryData<FullTextQueueCache>(fullTextQueueKey, (cache) =>
				applyFullTextState(cache, authoritative)
			);
			statusMessage = `Full-text ${decision} recorded.`;
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
			const authoritative = {
				report_id: result.data.report_id,
				full_text_status: result.data.full_text_status,
				full_text_exclusion_reason_id: result.data.full_text_exclusion_reason_id,
				revision: result.data.revision
			};
			queryClient.setQueryData<FullTextQueueCache>(fullTextQueueKey, (cache) =>
				applyFullTextState(cache, authoritative)
			);
			statusMessage = 'The last full-text decision was undone.';
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

	function attachExternal() {
		if (!currentReportId || !externalUrl.trim()) return;
		uploading = true;
		void attachExternalPdf(projectId, currentReportId, {
			url: externalUrl.trim(),
			original_filename: externalFilename.trim() || null
		})
			.then(async () => {
				statusMessage = 'External PDF queued for guarded retrieval and parsing.';
				externalUrl = '';
				externalFilename = '';
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
</script>

<div class="mx-auto flex w-full max-w-7xl flex-col gap-6 p-4 md:p-8">
	<header class="flex flex-col gap-3 lg:flex-row lg:items-end lg:justify-between">
		<div>
			<div class="text-sm text-muted-foreground">
				Evidence workspace / full-text screening
			</div>
			<h1 class="text-3xl font-semibold tracking-tight">Screen full text</h1>
			<p class="max-w-2xl text-muted-foreground">
				Review protocol criteria beside the source PDF and keep every decision auditable.
			</p>
		</div>
		<div class="flex items-center gap-2">
			<Button variant="outline" onclick={() => void move('previous')}>Previous</Button><Button
				variant="outline"
				onclick={() => void move('next')}>Next</Button
			>
		</div>
	</header>

	<div
		class="flex flex-wrap items-center gap-2 rounded-lg border bg-muted/20 p-3 text-sm"
		aria-label="Full-text queue filters"
	>
		<Button
			variant={urlState.filter === 'all' ? 'default' : 'outline'}
			onclick={() => void updateUrl({ filter: 'all', report: null })}>All included</Button
		>
		<Button
			variant={urlState.filter === 'available' ? 'default' : 'outline'}
			onclick={() => void updateUrl({ filter: 'available', report: null })}>Available</Button
		>
		<Button
			variant={urlState.filter === 'failed' ? 'default' : 'outline'}
			onclick={() => void updateUrl({ filter: 'failed', report: null })}>Failed</Button
		>
		<Button
			variant={urlState.filter === 'missing' ? 'default' : 'outline'}
			onclick={() => void updateUrl({ filter: 'missing', report: null })}
			>Missing / failed ({missingItems.length})</Button
		>
		{#if currentIndex >= 0}<Badge variant="secondary"
				>{currentIndex + 1} of {urlState.filter === 'missing'
					? missingItems.length
					: visibleQueueItems.length}
				loaded</Badge
			>{:else if urlState.filter === 'missing' && queueCurrent}<Badge variant="secondary"
				>Attached · left missing queue</Badge
			>{/if}
		{#if urlState.filter !== 'missing' && fullQueueQuery.hasNextPage}<Button
				variant="outline"
				disabled={fullQueueQuery.isFetchingNextPage}
				onclick={() => void loadMoreReports()}
				>{fullQueueQuery.isFetchingNextPage ? 'Loading more…' : 'Load more reports'}</Button
			>{/if}
	</div>

	{#if errorMessage}<Alert.Root variant="destructive" role="alert"
			><Alert.Title>Full-text review needs attention</Alert.Title><Alert.Description
				>{errorMessage}</Alert.Description
			></Alert.Root
		>{/if}
	{#if statusMessage}<div
			class="rounded-md border border-amber-500/40 bg-amber-500/10 p-3 text-sm"
			role="status"
			aria-live="polite"
		>
			{statusMessage}
		</div>{/if}

	{#if !current}
		<Card.Root
			><Card.Header><Card.Title>No included reports in this queue</Card.Title></Card.Header
			><Card.Content class="text-sm text-muted-foreground"
				>Include a report during title/abstract screening, or choose the missing-full-text
				filter.</Card.Content
			></Card.Root
		>
	{:else}
		<div class="grid gap-6 xl:grid-cols-[minmax(0,1fr)_24rem]">
			<main class="flex min-w-0 flex-col gap-6">
				<Card.Root>
					<Card.Header class="flex-row items-start justify-between gap-4"
						><div>
							<Card.Title>{current.title ?? 'Untitled report'}</Card.Title
							><Card.Description
								>{current.abstract_text ??
									'No abstract is available.'}</Card.Description
							>
						</div>
						<Badge variant="outline">{screenStatus.replaceAll('_', ' ')}</Badge
						></Card.Header
					>
					<Card.Content class="flex flex-col gap-4">
						{#if contentUrl && document?.status === 'available'}<PdfViewer
								{contentUrl}
								{blocks}
								pageMetadata={pages}
								selectedPage={urlState.page}
								{selectedBlockId}
								onBlockSelect={handleBlockSelect}
							/>{:else}<div
								class="rounded-lg border border-dashed p-8 text-center text-sm text-muted-foreground"
							>
								{document?.status
									? `Document status: ${document.status.replaceAll('_', ' ')}`
									: 'Attach a PDF to open the document viewer.'}
							</div>{/if}
						<div class="flex flex-wrap items-center gap-2">
							<label class="inline-flex cursor-pointer items-center"
								><span class="sr-only">Upload PDF</span><input
									class="hidden"
									type="file"
									accept="application/pdf,.pdf"
									onchange={upload}
									disabled={uploading}
								/><Button
									type="button"
									variant="outline"
									disabled={uploading}
									onclick={(event) => {
										const target =
											event.currentTarget.parentElement?.querySelector(
												'input'
											);
										if (target instanceof HTMLInputElement) target.click();
									}}>{uploading ? 'Uploading…' : 'Upload PDF'}</Button
								></label
							><Input
								bind:value={externalUrl}
								type="url"
								placeholder="https://…/article.pdf"
								aria-label="External PDF URL"
							/><Input
								bind:value={externalFilename}
								placeholder="Filename (optional)"
								aria-label="External PDF filename"
							/><Button
								type="button"
								variant="outline"
								disabled={uploading || !externalUrl.trim()}
								onclick={attachExternal}>Attach URL</Button
							>
						</div>
					</Card.Content>
				</Card.Root>

				<Card.Root
					><Card.Header
						><Card.Title>Evidence blocks</Card.Title><Card.Description
							>Parser {document?.parser_version ?? 'pending'}{document?.ocr_required
								? ' · OCR required on one or more pages'
								: ''}</Card.Description
						></Card.Header
					><Card.Content
						>{#if blocks.length === 0}<p class="text-sm text-muted-foreground">
								Evidence blocks appear after asynchronous parsing.
							</p>{:else}<ol
								class="flex max-h-72 flex-col gap-2 overflow-auto"
								aria-label="Parsed evidence blocks"
							>
								{#each blocks as block (block.id)}<li>
										<EvidenceLink
											{block}
											selected={selectedBlockId === block.id}
											onSelect={handleBlockSelect}
										/>
									</li>{/each}
							</ol>{/if}</Card.Content
					></Card.Root
				>

				<Card.Root
					><Card.Header><Card.Title>Full-text decision</Card.Title></Card.Header
					><Card.Content
						><FullTextDecisionBar
							reasons={reasons.filter((reason) => reason.stage === 'full_text')}
							{selectedReason}
							pending={decisionMutation.isPending || undoMutation.isPending}
							{canUndo}
							available={usableFullText}
							onReasonChange={(reasonId) => (selectedReason = reasonId)}
							onDecision={(decision, reasonId) => void decide(decision, reasonId)}
							onUndo={() => void undo()}
						/></Card.Content
					></Card.Root
				>
			</main>
			<aside class="flex flex-col gap-6">
				<CriteriaPanel
					criteria={protocol?.criteria ?? []}
					protocolVersion={protocol?.version}
					stage="full_text"
				/>
				{#if currentReportId}
					<AiProposalReview
						{projectId}
						reportId={currentReportId}
						stage="full_text"
						protocolVersionId={protocol?.id}
						expectedRevision={screenRevision}
						onEvidenceSelect={handleAiEvidenceSelect}
					/>
				{/if}
				<ScreeningHistory items={historyItems} />
			</aside>
		</div>
	{/if}
</div>
