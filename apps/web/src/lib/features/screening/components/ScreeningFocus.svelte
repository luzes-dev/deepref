<script lang="ts">
	import {
		createGetProjectProtocol,
		createGetScreeningHistory,
		createScreenReport,
		createUndoScreening,
		getGetScreeningHistoryQueryOptions,
		getScreeningQueue
	} from '$lib/api/generated/review/review';
	import type {
		ApiErrorBody,
		ScreeningQueueItemDto,
		ScreeningStateDto
	} from '$lib/api/generated/models';
	import { ApiError } from '$lib/api/custom-fetch';
	import { Button } from '@deepref/ui/button';
	import { Input } from '@deepref/ui/input';
	import { Skeleton } from '@deepref/ui/skeleton';
	import * as Resizable from '@deepref/ui/resizable';
	import { createInfiniteQuery, useQueryClient } from '@tanstack/svelte-query';
	import { ArrowLeft, ArrowRight, CheckCircle2, LayoutGrid, List, Search } from '@lucide/svelte';
	import { MediaQuery } from 'svelte/reactivity';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import type { ResolvedPathname } from '$app/types';
	import { page } from '$app/state';
	import AiProposalReview from '$lib/features/ai-assistance/components/AiProposalReview.svelte';
	import { canRequestAiSuggestions } from '$lib/features/ai-assistance/availability';
	import { createGetAiStatus } from '$lib/api/generated/ai/ai';
	import DecisionBar from './DecisionBar.svelte';
	import PageTemplate from '$lib/shell/PageTemplate.svelte';
	import CriteriaPanel from './CriteriaPanel.svelte';
	import ScreeningFeedback from './ScreeningFeedback.svelte';
	import ScreeningHistory from './ScreeningHistory.svelte';
	import ScreeningTable from './ScreeningTable.svelte';
	import { screeningKeys } from '../api';
	import {
		parseScreeningUrl,
		screeningUrlString,
		type ScreeningMode,
		type ScreeningStatus,
		type ScreeningUrlState
	} from '../filters';
	import {
		applyOptimisticStatusChange,
		applyServerStateToQueue,
		findQueueItem,
		findQueueLocation,
		type QueueLocation,
		type ScreeningQueueCache
	} from '../optimistic';
	import {
		hasCommandModifier,
		hasOpenScreeningOverlay,
		isShortcutSuppressed,
		shortcutAction
	} from '../shortcuts';

	type QueueCache = ScreeningQueueCache;

	type LastAction = {
		reportId: string;
		postDecisionItem: ScreeningQueueItemDto;
		location: QueueLocation | null;
		priorStatus: string;
		returnedRevision: number;
	};
	type ScreeningPath =
		| `/projects/${string}/screening/title-abstract`
		| `/projects/${string}/screening/title-abstract?${string}`;
	let { projectId }: { projectId: string } = $props();

	// Wait for status before showing suggestions, while failing open if status cannot load.
	const aiStatusQuery = createGetAiStatus();
	const aiSuggestionsAvailable = $derived(canRequestAiSuggestions(aiStatusQuery));
	const queryClient = useQueryClient();

	const urlState = $derived(parseScreeningUrl(page.url.searchParams));
	const queueKey = $derived([
		'screening-queue',
		projectId,
		urlState.status,
		urlState.search,
		urlState.sort
	] as const);
	const queueQuery = createInfiniteQuery(() => ({
		queryKey: queueKey,
		initialPageParam: undefined as string | undefined,
		queryFn: ({ pageParam, signal }) =>
			getScreeningQueue(
				projectId,
				{
					status: urlState.status,
					search: urlState.search || undefined,
					sort: urlState.sort,
					cursor: pageParam,
					limit: 25
				},
				{ signal }
			),
		getNextPageParam: (lastPage) => lastPage.data.next_cursor ?? undefined
	}));
	const protocolQuery = createGetProjectProtocol(() => projectId);
	const decisionMutation = createScreenReport();
	const undoMutation = createUndoScreening();
	let statusMessage = $state('');
	let lastAction = $state<LastAction | null>(null);
	let searchTimer: ReturnType<typeof setTimeout> | undefined;

	const pages = $derived(queueQuery.data?.pages ?? []);
	const queueItems = $derived(pages.flatMap((page) => page.data.items));
	const firstPage = $derived(pages[0]?.data);
	const selectedReportId = $derived(urlState.report ?? queueItems[0]?.report_id ?? null);
	const historyQuery = createGetScreeningHistory(
		() => projectId,
		() => selectedReportId ?? '',
		() => ({ query: { enabled: Boolean(selectedReportId) } })
	);
	const currentIndex = $derived(
		selectedReportId ? queueItems.findIndex((item) => item.report_id === selectedReportId) : 0
	);
	const current = $derived(queueItems[currentIndex] ?? queueItems[0] ?? null);
	const protocol = $derived(protocolQuery.data?.data);
	const historyItems = $derived(historyQuery.data?.data.items ?? []);
	const latestHistory = $derived(historyItems.at(-1) ?? null);
	const historyUndoAvailable = $derived(
		Boolean(
			current &&
			latestHistory?.stage === 'title_abstract' &&
			latestHistory.event_kind === 'decision'
		)
	);
	const undoTarget = $derived(
		lastAction
			? {
					reportId: lastAction.reportId,
					item: lastAction.postDecisionItem,
					location: lastAction.location,
					expectedRevision: lastAction.returnedRevision,
					restoreStatus: lastAction.priorStatus
				}
			: historyUndoAvailable && current && latestHistory
				? {
						reportId: current.report_id,
						item: current,
						location: null,
						expectedRevision: current.revision,
						restoreStatus: latestHistory.previous_title_abstract_status
					}
				: null
	);
	const canUndo = $derived(Boolean(undoTarget));
	const progress = $derived(
		firstPage?.progress ?? {
			total: 0,
			screened: 0,
			unscreened: 0,
			included: 0,
			excluded: 0,
			maybe: 0
		}
	);
	const errorMessage = $derived(
		(decisionMutation.error as Error | null)?.message ||
			(undoMutation.error as Error | null)?.message ||
			(queueQuery.error as Error | null)?.message ||
			(protocolQuery.error as Error | null)?.message
	);
	const progressPercent = $derived(
		progress.total ? Math.round((progress.screened / progress.total) * 100) : 0
	);
	const queueCount = $derived(firstPage?.total ?? queueItems.length);

	const wide = new MediaQuery('(min-width: 1024px)');
	const statusOptions = $derived<{ value: ScreeningStatus; label: string; count: number }[]>([
		{ value: 'unscreened', label: 'To screen', count: progress.unscreened },
		{ value: 'maybe', label: 'Maybe', count: progress.maybe },
		{ value: 'include', label: 'Included', count: progress.included },
		{ value: 'exclude', label: 'Excluded', count: progress.excluded },
		{ value: 'all', label: 'All', count: progress.total }
	]);

	function decisionLabel(value: string) {
		return value === 'include' ? 'Included' : value === 'exclude' ? 'Excluded' : 'Maybe';
	}

	$effect(() => {
		return () => {
			if (searchTimer) clearTimeout(searchTimer);
		};
	});

	$effect(() => {
		if (
			urlState.mode === 'focus' &&
			urlState.report &&
			!current &&
			queueQuery.hasNextPage &&
			!queueQuery.isFetchingNextPage
		) {
			void queueQuery.fetchNextPage();
		}
	});

	$effect(() => {
		if (!current || urlState.mode !== 'focus') return;
		const nextItems = queueItems.slice(Math.max(currentIndex + 1, 0), currentIndex + 6);
		for (const item of nextItems) {
			void queryClient.prefetchQuery(
				getGetScreeningHistoryQueryOptions(projectId, item.report_id)
			);
		}
	});

	function nextUrlState(changes: Partial<ScreeningUrlState>): ScreeningUrlState {
		return { ...urlState, ...changes };
	}

	function appendSearch(pathname: ResolvedPathname, search: string): ScreeningPath {
		return `${pathname}${search}` as ScreeningPath;
	}

	function navigateTo(url: ScreeningPath, options: Parameters<typeof goto>[1]) {
		return goto(resolve(url), options);
	}

	async function updateUrl(changes: Partial<ScreeningUrlState>, replaceState = true) {
		await navigateTo(
			appendSearch(
				resolve('/projects/[projectId]/screening/title-abstract', { projectId }),
				screeningUrlString(nextUrlState(changes))
			),
			{
				replaceState,
				keepFocus: true,
				noScroll: true
			}
		);
	}

	function isAuthoritativeState(value: unknown, reportId: string): value is ScreeningStateDto {
		if (!value || typeof value !== 'object') return false;
		const state = value as Partial<ScreeningStateDto>;
		const titleStatuses = ['unscreened', 'include', 'exclude', 'maybe'];
		const fullTextStatuses = ['not_required', 'unscreened', 'include', 'exclude', 'maybe'];
		if (state.project_id !== projectId || state.report_id !== reportId) return false;
		if (!titleStatuses.includes(state.title_abstract_status ?? '')) return false;
		if (!fullTextStatuses.includes(state.full_text_status ?? '')) return false;
		if (typeof state.final_status !== 'string' || state.final_status.length === 0) return false;
		if (!Number.isInteger(state.revision) || (state.revision ?? -1) < 0) return false;
		const reason = state.full_text_exclusion_reason_id;
		if (state.full_text_status === 'exclude' && typeof reason !== 'string') return false;
		if (state.full_text_status !== 'exclude' && reason != null) return false;
		if (state.title_abstract_status !== 'include' && state.full_text_status !== 'not_required')
			return false;
		return true;
	}

	function currentStateFromError(error: unknown, reportId: string): ScreeningStateDto | null {
		if (!(error instanceof ApiError)) return null;
		const info = error.info as ApiErrorBody | null;
		const details = info?.details;
		const state = details && typeof details === 'object' ? details.currentState : null;
		return isAuthoritativeState(state, reportId) ? state : null;
	}

	async function invalidateScreeningReport(reportId: string) {
		await queryClient.invalidateQueries({ queryKey: screeningKeys.queue(projectId) });
		await queryClient.invalidateQueries({
			queryKey: screeningKeys.history(projectId, reportId)
		});
	}

	async function reconcileConflict(
		reportId: string,
		error: unknown,
		oldCache: QueueCache | undefined,
		fallbackItem: ScreeningQueueItemDto,
		priorLocation: QueueLocation | null
	) {
		queryClient.setQueryData(queueKey, oldCache);
		const state = currentStateFromError(error, reportId);
		if (state) {
			const allKey = [
				'screening-queue',
				projectId,
				'all',
				urlState.search,
				urlState.sort
			] as const;
			queryClient.setQueryData<QueueCache>(allKey, (cache) =>
				applyServerStateToQueue(
					cache ?? oldCache,
					state,
					'all',
					fallbackItem,
					priorLocation
				)
			);
		}
		await updateUrl({ mode: 'focus', status: 'all', report: reportId }, true);
		await invalidateScreeningReport(reportId);
		decisionMutation.reset();
		undoMutation.reset();
		lastAction = null;
		statusMessage = state
			? 'This report changed elsewhere. The authoritative server state is shown; review it before deciding again.'
			: 'This report changed elsewhere. The queue was refreshed; review the current state before deciding again.';
	}

	async function decide(decision: 'include' | 'exclude' | 'maybe') {
		if (!current || !protocol || decisionMutation.isPending || undoMutation.isPending) return;
		statusMessage = '';
		const reportId = current.report_id;
		const oldCache = queryClient.getQueryData<QueueCache>(queueKey);
		const priorLocation = findQueueLocation(oldCache, reportId);
		const priorItem = { ...current };
		const expectedRevision = current.revision;
		const nextItem = queueItems[currentIndex + 1] ?? queueItems[0] ?? null;
		queryClient.setQueryData<QueueCache>(queueKey, (cache) =>
			applyOptimisticStatusChange(cache, {
				reportId,
				fromStatus: current.title_abstract_status,
				toStatus: decision,
				revision: expectedRevision + 1,
				filterStatus: urlState.status,
				priorItem,
				priorLocation
			})
		);
		await updateUrl({ report: nextItem?.report_id ?? null }, false);
		try {
			const result = await decisionMutation.mutateAsync({
				projectId,
				reportId,
				data: {
					stage: 'title_abstract',
					decision,
					protocol_version_id: protocol.id,
					expected_revision: expectedRevision
				}
			});
			lastAction = {
				reportId,
				postDecisionItem: {
					...priorItem,
					title_abstract_status: decision,
					full_text_status: result.data.full_text_status,
					final_status: result.data.final_status,
					revision: result.data.revision
				},
				location: priorLocation,
				priorStatus: priorItem.title_abstract_status,
				returnedRevision: result.data.revision
			};
			await invalidateScreeningReport(reportId);
		} catch (error) {
			if (error instanceof ApiError && error.status === 409) {
				await reconcileConflict(reportId, error, oldCache, priorItem, priorLocation);
			} else {
				queryClient.setQueryData(queueKey, oldCache);
				await updateUrl({ report: reportId }, true);
			}
		}
	}

	async function undo() {
		if (!protocol || !undoTarget || undoMutation.isPending || decisionMutation.isPending)
			return;
		statusMessage = '';
		const target = undoTarget;
		const reportId = target.reportId;
		const previousReport = urlState.report;
		const oldCache = queryClient.getQueryData<QueueCache>(queueKey);
		const existingItem = findQueueItem(oldCache, reportId) ?? target.item;
		const priorLocation = findQueueLocation(oldCache, reportId) ?? target.location;
		queryClient.setQueryData<QueueCache>(queueKey, (cache) =>
			applyOptimisticStatusChange(cache, {
				reportId,
				fromStatus: existingItem.title_abstract_status,
				toStatus: target.restoreStatus,
				revision: target.expectedRevision + 1,
				filterStatus: urlState.status,
				priorItem: existingItem,
				priorLocation
			})
		);
		await updateUrl({ mode: 'focus', report: reportId }, false);
		try {
			const result = await undoMutation.mutateAsync({
				projectId,
				reportId,
				data: {
					stage: 'title_abstract',
					protocol_version_id: protocol.id,
					expected_revision: target.expectedRevision
				}
			});
			lastAction = null;
			queryClient.setQueryData<QueueCache>(queueKey, (cache) =>
				applyServerStateToQueue(
					cache,
					result.data,
					urlState.status,
					existingItem,
					priorLocation
				)
			);
			await invalidateScreeningReport(reportId);
		} catch (error) {
			if (error instanceof ApiError && error.status === 409) {
				await reconcileConflict(reportId, error, oldCache, existingItem, priorLocation);
			} else {
				queryClient.setQueryData(queueKey, oldCache);
				if (lastAction?.reportId === reportId)
					await updateUrl({ report: previousReport }, true);
			}
		}
	}

	async function selectReport(reportId: string, push = true) {
		await updateUrl({ mode: 'focus', report: reportId }, !push);
	}

	async function move(direction: 'previous' | 'next') {
		if (queueItems.length === 0) return;
		if (
			direction === 'next' &&
			currentIndex === queueItems.length - 1 &&
			queueQuery.hasNextPage
		) {
			await queueQuery.fetchNextPage();
		}
		const latestItems = queueQuery.data?.pages.flatMap((page) => page.data.items) ?? queueItems;
		const index = latestItems.findIndex((item) => item.report_id === current?.report_id);
		const nextIndex = direction === 'next' ? index + 1 : index - 1;
		const item =
			latestItems[nextIndex] ?? (direction === 'previous' ? latestItems.at(-1) : null);
		if (item) await updateUrl({ report: item.report_id }, false);
	}

	function changeMode(mode: ScreeningMode) {
		void updateUrl({ mode }, false);
	}

	function changeStatus(status: ScreeningStatus) {
		void updateUrl({ status, report: null }, false);
	}

	function scheduleSearch(search: string) {
		if (searchTimer) clearTimeout(searchTimer);
		searchTimer = setTimeout(() => {
			void updateUrl({ search, report: null }, true);
		}, 250);
	}

	function handleKeydown(event: KeyboardEvent) {
		const overlayOpen = hasOpenScreeningOverlay();
		if (
			event.defaultPrevented ||
			hasCommandModifier(event) ||
			isShortcutSuppressed(event.target, overlayOpen)
		)
			return;
		const action = shortcutAction(event.key);
		if (!action) return;
		event.preventDefault();
		if (action === 'include' || action === 'exclude' || action === 'maybe') void decide(action);
		if (action === 'undo') void undo();
		if (action === 'previous' || action === 'next') void move(action);
	}
</script>

<svelte:window onkeydown={handleKeydown} />

{#snippet queueControls()}
	<div class="flex flex-col gap-2">
		<div class="relative">
			<Search
				class="pointer-events-none absolute top-1/2 left-2.5 size-4 -translate-y-1/2 text-muted-foreground"
				aria-hidden="true"
			/>
			<Input
				id="screening-search"
				aria-label="Search title or abstract"
				class="h-8 pl-8"
				value={urlState.search}
				placeholder="Search reports"
				oninput={(event) => scheduleSearch(event.currentTarget.value)}
			/>
		</div>
		<div class="flex flex-wrap gap-1" role="group" aria-label="Queue status">
			{#each statusOptions as option (option.value)}
				<button
					type="button"
					class={[
						'inline-flex h-7 shrink-0 items-center gap-1.5 rounded-md px-2 text-xs font-medium whitespace-nowrap transition-colors focus-visible:outline-2 focus-visible:outline-ring',
						urlState.status === option.value
							? 'bg-primary text-primary-foreground'
							: 'text-muted-foreground hover:bg-muted hover:text-foreground'
					]}
					aria-pressed={urlState.status === option.value}
					onclick={() => changeStatus(option.value)}
					>{option.label}<span class="tabular-nums opacity-70">{option.count}</span
					></button
				>
			{/each}
		</div>
	</div>
{/snippet}

{#snippet queueFooter()}
	<div class="flex items-center justify-between gap-2 text-xs text-muted-foreground">
		<label class="flex items-center gap-1.5" for="screening-sort">
			<span>Sort</span>
			<select
				id="screening-sort"
				class="h-7 rounded-md bg-transparent px-1 text-xs text-foreground outline-none hover:bg-muted focus-visible:ring-2 focus-visible:ring-ring"
				value={urlState.sort}
				onchange={(event) =>
					void updateUrl(
						{
							sort: event.currentTarget.value as ScreeningUrlState['sort'],
							report: null
						},
						false
					)}
			>
				<option value="created_asc">Oldest first</option>
				<option value="created_desc">Newest first</option>
				<option value="title_asc">Title A–Z</option>
				<option value="title_desc">Title Z–A</option>
				<option value="year_asc">Year ascending</option>
				<option value="year_desc">Year descending</option>
			</select>
		</label>
		<Button
			variant="ghost"
			size="xs"
			aria-pressed={urlState.mode === 'table'}
			onclick={() => changeMode(urlState.mode === 'table' ? 'focus' : 'table')}
			>{#if urlState.mode === 'table'}<List data-icon="inline-start" /> Focus{:else}<LayoutGrid
					data-icon="inline-start"
				/> Table{/if}</Button
		>
	</div>
{/snippet}

{#snippet queueList()}
	{#if queueQuery.isPending}
		<div class="flex flex-col gap-2 p-2">
			{#each { length: 5 }, index (index)}<Skeleton class="h-12 w-full" />{/each}
		</div>
	{:else if queueItems.length === 0}
		<p class="p-3 text-xs text-muted-foreground">Nothing in this view.</p>
	{:else}
		<ol class="flex flex-col p-1.5" aria-label="Queue list">
			{#each queueItems as item (item.report_id)}
				{@const selected = item.report_id === current?.report_id}
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
							<span class="tabular-nums">{item.publication_year ?? '—'}</span>
							{#if item.title_abstract_status !== 'unscreened'}
								<span class="flex items-center gap-1">
									<span
										class={[
											'size-1.5 rounded-full',
											item.title_abstract_status === 'include' &&
												'bg-success',
											item.title_abstract_status === 'exclude' &&
												'bg-destructive',
											item.title_abstract_status === 'maybe' && 'bg-warning'
										]}
										aria-hidden="true"
									></span>{decisionLabel(item.title_abstract_status)}
								</span>
							{/if}
						</span>
					</button>
				</li>
			{/each}
			{#if queueQuery.hasNextPage}
				<li class="p-1">
					<Button
						variant="ghost"
						size="sm"
						class="w-full"
						disabled={queueQuery.isFetchingNextPage}
						onclick={() => void queueQuery.fetchNextPage()}>Load more</Button
					>
				</li>
			{/if}
		</ol>
	{/if}
{/snippet}

{#snippet emptyReader()}
	<div class="flex flex-col items-start gap-4 py-10" data-testid="screening-empty">
		{#if (urlState.status === 'unscreened' || urlState.status === 'maybe') && !urlState.search && progress.total > 0 && progress.unscreened === 0}
			<CheckCircle2 class="size-6 text-success" aria-hidden="true" />
			<div class="flex flex-col gap-1">
				<h2 class="editorial-title text-xl">
					{progress.maybe > 0
						? `All records screened, ${progress.maybe} still maybe`
						: `All ${progress.total.toLocaleString()} records screened`}
				</h2>
				<p class="text-sm text-muted-foreground">
					{progress.included} included · {progress.excluded} excluded · {progress.maybe} maybe
				</p>
			</div>
			<div class="flex flex-wrap gap-2">
				{#if progress.maybe > 0}
					<Button onclick={() => changeStatus('maybe')}
						>Resolve {progress.maybe} maybe{progress.maybe === 1 ? '' : 's'}</Button
					>
				{/if}
				<Button
					variant={progress.maybe > 0 ? 'outline' : 'default'}
					href={resolve('/projects/[projectId]/screening/full-text', { projectId })}
					>Continue to full text<ArrowRight data-icon="inline-end" /></Button
				>
				{#if canUndo}<Button variant="ghost" onclick={() => void undo()}
						>Undo last decision</Button
					>{/if}
			</div>
		{:else}
			<h2 class="editorial-title text-xl">Nothing to show</h2>
			<p class="text-sm text-muted-foreground">
				No reports match {urlState.search ? `“${urlState.search}” in ` : ''}this view.
			</p>
			<div class="flex flex-wrap gap-2">
				<Button variant="outline" onclick={() => changeStatus('all')}
					>Show all reports</Button
				>
				{#if canUndo}<Button variant="ghost" onclick={() => void undo()}
						>Undo last decision</Button
					>{/if}
			</div>
		{/if}
	</div>
{/snippet}

{#snippet readerContent()}
	{#if queueQuery.isPending}
		<div class="flex flex-col gap-4 py-8" aria-label="Loading reports" aria-live="polite">
			<Skeleton class="h-4 w-32" /><Skeleton class="h-10 w-4/5" /><Skeleton
				class="h-40 w-full"
			/>
		</div>
	{:else if !current}
		{@render emptyReader()}
	{:else}
		<article class="flex max-w-3xl flex-col gap-5 py-6" aria-live="polite">
			<div class="flex flex-col gap-2">
				<p class="flex flex-wrap gap-x-2 text-xs text-muted-foreground">
					<span class="tabular-nums">{current.publication_year ?? 'Year unknown'}</span>
					{#if current.doi}<span aria-hidden="true">·</span><span class="truncate"
							>{current.doi}</span
						>{/if}
				</p>
				<h2 class="editorial-title text-2xl leading-tight sm:text-3xl">
					{current.title ?? 'Untitled report'}
				</h2>
			</div>
			<p class="text-base leading-7 whitespace-pre-wrap text-foreground/90">
				{current.abstract_text ??
					'No abstract is available. Use Maybe when the available evidence is insufficient.'}
			</p>
			{#if aiSuggestionsAvailable}
				<details class="disclosure">
					<summary>Get an AI suggestion</summary>
					<AiProposalReview
						{projectId}
						reportId={current.report_id}
						stage="title_abstract"
						protocolVersionId={protocol?.id}
						expectedRevision={current.revision}
					/>
				</details>
			{/if}
		</article>
	{/if}
{/snippet}

{#snippet decisionBar()}
	{#if current}
		<DecisionBar
			disabled={!protocol}
			pending={decisionMutation.isPending || undoMutation.isPending}
			current={current.title_abstract_status}
			{canUndo}
			onDecision={decide}
			onUndo={undo}
		/>
	{/if}
{/snippet}

{#snippet position()}
	<div class="flex items-center gap-1">
		<span class="mr-1 text-xs text-muted-foreground tabular-nums">
			{#if current}{Math.max(currentIndex + 1, 1)} of {queueCount}{:else}{queueCount} in view{/if}
		</span>
		<Button
			variant="ghost"
			size="icon-sm"
			aria-label="Previous report (ArrowLeft)"
			disabled={!current}
			onclick={() => void move('previous')}><ArrowLeft aria-hidden="true" /></Button
		>
		<Button
			variant="ghost"
			size="icon-sm"
			aria-label="Next report (ArrowRight)"
			disabled={!current}
			onclick={() => void move('next')}><ArrowRight aria-hidden="true" /></Button
		>
	</div>
{/snippet}

{#snippet progressLine()}
	<div
		class="flex items-center gap-3 text-xs text-muted-foreground"
		aria-label="Screening progress"
	>
		<div
			class="h-1 w-24 overflow-hidden rounded-full bg-muted"
			role="progressbar"
			aria-label="Screening progress"
			aria-valuemin="0"
			aria-valuemax={Math.max(1, progress.total)}
			aria-valuenow={progress.screened}
		>
			<div class="h-full bg-primary" style:width={`${progressPercent}%`}></div>
		</div>
		<span class="tabular-nums">{progress.screened} of {progress.total} screened</span>
	</div>
{/snippet}

{#snippet context()}
	<CriteriaPanel criteria={protocol?.criteria ?? []} protocolVersion={protocol?.version} />
	{#if current}<ScreeningHistory items={historyItems} />{/if}
	<p class="text-xs leading-5 text-muted-foreground">
		<kbd>I</kbd> include · <kbd>E</kbd> exclude · <kbd>M</kbd> maybe · <kbd>U</kbd> undo ·
		<kbd>←</kbd>
		<kbd>→</kbd> move
	</p>
{/snippet}

{#if wide.current}
	<div class="flex min-h-0 flex-1 flex-col" data-testid="screening-page">
		<ScreeningFeedback
			errorTitle="Screening could not continue"
			{errorMessage}
			{statusMessage}
		/>
		<Resizable.PaneGroup
			direction="horizontal"
			class="min-h-0 flex-1"
			autoSaveId="deepref:screening-layout"
		>
			<Resizable.Pane order={1} defaultSize={24} minSize={16} maxSize={40}>
				<aside class="flex h-full min-h-0 flex-col" aria-label="Screening queue">
					<div class="border-b p-3">{@render queueControls()}</div>
					<div class="min-h-0 flex-1 overflow-y-auto">{@render queueList()}</div>
					<div class="border-t px-3 py-1.5">{@render queueFooter()}</div>
				</aside>
			</Resizable.Pane>
			<Resizable.Handle />
			<Resizable.Pane order={2} defaultSize={52} minSize={34}>
				{#if urlState.mode === 'table'}
					<div class="h-full overflow-y-auto p-4">
						<ScreeningTable
							items={queueItems}
							selectedReport={selectedReportId}
							loading={queueQuery.isPending}
							hasNextPage={queueQuery.hasNextPage ?? false}
							loadingNextPage={queueQuery.isFetchingNextPage}
							onSelect={selectReport}
							onLoadMore={async () => {
								await queueQuery.fetchNextPage();
							}}
						/>
					</div>
				{:else}
					<section
						class="flex h-full min-h-0 flex-col"
						aria-label="Report"
						data-testid="screening-focus"
					>
						<div class="flex items-center justify-between gap-3 border-b px-6 py-1.5">
							{@render progressLine()}
							{@render position()}
						</div>
						<div class="min-h-0 flex-1 overflow-y-auto px-6">
							{@render readerContent()}
						</div>
						{#if current}<div class="border-t px-6 py-3">
								{@render decisionBar()}
							</div>{/if}
					</section>
				{/if}
			</Resizable.Pane>
			<Resizable.Handle />
			<Resizable.Pane order={3} defaultSize={24} minSize={16} maxSize={36}>
				<aside
					class="flex h-full min-h-0 flex-col gap-8 overflow-y-auto px-5 py-5"
					aria-label="Criteria and history"
				>
					{@render context()}
				</aside>
			</Resizable.Pane>
		</Resizable.PaneGroup>
	</div>
{:else}
	<PageTemplate testId="screening-page" containerClass="gap-4">
		{@render queueControls()}
		<ScreeningFeedback
			errorTitle="Screening could not continue"
			{errorMessage}
			{statusMessage}
		/>
		<section class="flex flex-col" aria-label="Report" data-testid="screening-focus">
			<div class="flex items-center justify-between gap-3 border-b pb-2">
				{@render progressLine()}
				{@render position()}
			</div>
			{@render readerContent()}
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

<style>
	kbd {
		display: inline-flex;
		min-width: 1.5rem;
		align-items: center;
		justify-content: center;
		border: 1px solid color-mix(in oklab, var(--border) 80%, transparent);
		border-radius: 0.3rem;
		background: var(--muted);
		padding: 0.1rem 0.35rem;
		font-family: var(--font-sans);
		font-size: 0.6875rem;
		font-weight: 600;
		color: var(--foreground);
	}
</style>
