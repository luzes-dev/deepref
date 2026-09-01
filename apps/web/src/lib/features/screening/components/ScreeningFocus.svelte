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
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';
	import * as Empty from '$lib/components/ui/empty';
	import { Input } from '$lib/components/ui/input';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { createInfiniteQuery, useQueryClient } from '@tanstack/svelte-query';
	import {
		ArrowLeft,
		ArrowRight,
		CheckCircle2,
		FileText,
		LayoutGrid,
		List,
		Search,
		ShieldCheck,
		SlidersHorizontal
	} from '@lucide/svelte';
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import type { ResolvedPathname } from '$app/types';
	import { page } from '$app/state';
	import AiProposalReview from '$lib/features/ai-assistance/components/AiProposalReview.svelte';
	import DecisionBar from './DecisionBar.svelte';
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
	import { hasOpenScreeningOverlay, isShortcutSuppressed, shortcutAction } from '../shortcuts';

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

	function statusLabel(value: string) {
		return value.replaceAll('_', ' ');
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
		if (event.defaultPrevented || isShortcutSuppressed(event.target, overlayOpen)) return;
		const action = shortcutAction(event.key);
		if (!action) return;
		event.preventDefault();
		if (action === 'include' || action === 'exclude' || action === 'maybe') void decide(action);
		if (action === 'undo') void undo();
		if (action === 'previous' || action === 'next') void move(action);
	}
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="mx-auto flex w-full max-w-[1480px] flex-col gap-5 p-4 md:gap-6 md:p-8">
	<header class="flex flex-col gap-4 lg:flex-row lg:items-end lg:justify-between">
		<div class="flex min-w-0 flex-col gap-2">
			<div
				class="flex flex-wrap items-center gap-2 text-xs font-semibold tracking-[0.12em] text-primary uppercase"
			>
				<ShieldCheck aria-hidden="true" /> Evidence workspace
				<span class="text-muted-foreground">/</span> title & abstract
			</div>
			<h1 class="editorial-title text-4xl leading-none sm:text-5xl">Screen reports</h1>
			<p class="max-w-2xl text-sm leading-6 text-muted-foreground sm:text-base">
				Review one report at a time against the published protocol. Decisions stay
				reversible and auditable.
			</p>
		</div>
		<div class="flex flex-wrap items-center gap-2 lg:justify-end">
			<Badge variant="secondary"
				>{progress.screened} screened · {progress.unscreened} pending</Badge
			>
			{#if protocol}
				<Badge variant="outline"
					><CheckCircle2 data-icon="inline-start" /> Protocol v{protocol.version} published</Badge
				>
			{:else}
				<Badge variant="outline">Loading protocol…</Badge>
			{/if}
		</div>
	</header>

	<section
		class="grid gap-4 rounded-xl border bg-card p-4 shadow-xs sm:grid-cols-[minmax(0,1fr)_auto] sm:items-center sm:p-5"
		aria-label="Screening progress"
	>
		<div class="flex min-w-0 flex-col gap-3">
			<div class="flex flex-wrap items-center justify-between gap-2">
				<div>
					<p
						class="text-xs font-semibold tracking-[0.12em] text-muted-foreground uppercase"
					>
						Review progress
					</p>
					<p class="mt-1 text-sm font-medium">
						{progress.screened} of {progress.total} reports screened
					</p>
				</div>
				<span class="text-2xl font-semibold tracking-tight text-primary"
					>{progressPercent}%</span
				>
			</div>
			<div
				class="h-2.5 overflow-hidden rounded-full bg-muted"
				role="progressbar"
				aria-valuemin="0"
				aria-valuemax={progress.total}
				aria-valuenow={progress.screened}
			>
				<div
					class="h-full rounded-full bg-primary"
					style={`width: ${progressPercent}%`}
				></div>
			</div>
		</div>
		<div
			class="grid grid-cols-4 divide-x rounded-lg border bg-muted/20 text-center sm:min-w-[24rem]"
		>
			<div class="flex flex-col gap-1 px-2 py-2">
				<span class="text-lg font-semibold">{progress.unscreened}</span><span
					class="text-[10px] tracking-wide text-muted-foreground uppercase">Pending</span
				>
			</div>
			<div class="flex flex-col gap-1 px-2 py-2">
				<span class="text-lg font-semibold text-success">{progress.included}</span><span
					class="text-[10px] tracking-wide text-muted-foreground uppercase">Include</span
				>
			</div>
			<div class="flex flex-col gap-1 px-2 py-2">
				<span class="text-lg font-semibold text-destructive">{progress.excluded}</span><span
					class="text-[10px] tracking-wide text-muted-foreground uppercase">Exclude</span
				>
			</div>
			<div class="flex flex-col gap-1 px-2 py-2">
				<span class="text-lg font-semibold text-warning">{progress.maybe}</span><span
					class="text-[10px] tracking-wide text-muted-foreground uppercase">Maybe</span
				>
			</div>
		</div>
	</section>

	<section
		class="flex flex-col gap-3 rounded-xl border bg-muted/20 p-3 md:p-4"
		aria-label="Screening queue filters"
	>
		<div class="flex flex-wrap items-center justify-between gap-2">
			<div class="flex items-center gap-2 text-sm font-semibold">
				<SlidersHorizontal aria-hidden="true" /> Queue filters
			</div>
			<span class="text-xs text-muted-foreground"
				>{queueCount} {queueCount === 1 ? 'report' : 'reports'} in view</span
			>
		</div>
		<div class="grid gap-3 lg:grid-cols-[minmax(16rem,1fr)_10rem_11rem_auto] lg:items-end">
			<label
				class="flex min-w-0 flex-col gap-1.5 text-xs font-semibold"
				for="screening-search"
			>
				<span>Search title or abstract</span>
				<div class="relative">
					<Search
						class="pointer-events-none absolute top-1/2 left-3 -translate-y-1/2 text-muted-foreground"
						aria-hidden="true"
					/>
					<Input
						id="screening-search"
						class="pl-9"
						value={urlState.search}
						placeholder="Search reports…"
						oninput={(event) => scheduleSearch(event.currentTarget.value)}
					/>
				</div>
			</label>
			<label class="flex flex-col gap-1.5 text-xs font-semibold" for="screening-status">
				<span>Status</span>
				<select
					id="screening-status"
					class="h-9 w-full rounded-md border bg-background px-3 text-sm font-normal outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
					value={urlState.status}
					onchange={(event) => changeStatus(event.currentTarget.value as ScreeningStatus)}
				>
					<option value="unscreened">Unscreened</option>
					<option value="include">Included</option>
					<option value="exclude">Excluded</option>
					<option value="maybe">Maybe</option>
					<option value="all">All</option>
				</select>
			</label>
			<label class="flex flex-col gap-1.5 text-xs font-semibold" for="screening-sort">
				<span>Sort by</span>
				<select
					id="screening-sort"
					class="h-9 w-full rounded-md border bg-background px-3 text-sm font-normal outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
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
			<div class="flex min-w-0 flex-col gap-1.5 text-xs font-semibold">
				<span>View</span>
				<div
					class="flex rounded-md border bg-background p-0.5"
					aria-label="Screening view mode"
				>
					<Button
						class="min-h-8 flex-1"
						size="sm"
						variant={urlState.mode === 'focus' ? 'default' : 'ghost'}
						aria-pressed={urlState.mode === 'focus'}
						onclick={() => changeMode('focus')}
						><List data-icon="inline-start" /> Focus</Button
					>
					<Button
						class="min-h-8 flex-1"
						size="sm"
						variant={urlState.mode === 'table' ? 'default' : 'ghost'}
						aria-pressed={urlState.mode === 'table'}
						onclick={() => changeMode('table')}
						><LayoutGrid data-icon="inline-start" /> Table</Button
					>
				</div>
			</div>
		</div>
	</section>

	<ScreeningFeedback errorTitle="Screening could not continue" {errorMessage} {statusMessage} />

	{#if urlState.mode === 'table'}
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
	{:else}
		<div class="grid gap-6 lg:grid-cols-[minmax(0,1fr)_22rem]">
			<Card.Root class="min-w-0 border-primary/15" data-testid="screening-focus">
				<Card.Header class="gap-3 border-b border-border/60 pb-4">
					<div class="flex flex-wrap items-start justify-between gap-3">
						<div class="flex items-center gap-2">
							<span
								class="flex size-8 items-center justify-center rounded-lg bg-primary/10 text-primary"
								><FileText aria-hidden="true" /></span
							>
							<div>
								<Card.Title>Focus mode</Card.Title>
								<Card.Description
									>{#if current}Report {Math.max(currentIndex + 1, 1)} of {queueCount}{:else}Your
										title/abstract queue{/if}</Card.Description
								>
							</div>
						</div>
						<div class="flex items-center gap-1">
							<Button
								variant="outline"
								size="icon"
								aria-label="Previous report (ArrowLeft)"
								onclick={() => void move('previous')}
								><ArrowLeft aria-hidden="true" /></Button
							>
							<Button
								variant="outline"
								size="icon"
								aria-label="Next report (ArrowRight)"
								onclick={() => void move('next')}
								><ArrowRight aria-hidden="true" /></Button
							>
						</div>
					</div>
				</Card.Header>
				<Card.Content class="flex flex-col gap-6 pt-5">
					{#if queueQuery.isPending}
						<div
							class="flex flex-col gap-4"
							aria-label="Loading reports"
							aria-live="polite"
						>
							<Skeleton class="h-5 w-32" /><Skeleton class="h-12 w-4/5" /><Skeleton
								class="h-40 w-full"
							/>
						</div>
					{:else if !current}
						<Empty.Root class="min-h-[24rem] border-dashed">
							<Empty.Media variant="icon"><FileText /></Empty.Media>
							<Empty.Header>
								<Empty.Title>Queue complete</Empty.Title>
								<Empty.Description
									>No reports match the current queue filters. Try a different
									status or search term.</Empty.Description
								>
							</Empty.Header>
						</Empty.Root>
					{:else}
						<article class="flex flex-col gap-5" aria-live="polite">
							<div class="flex flex-col gap-3">
								<div class="flex flex-wrap items-center justify-between gap-2">
									<div class="flex flex-wrap items-center gap-2">
										<Badge variant="outline"
											>Report {Math.max(currentIndex + 1, 1)}</Badge
										>
										<Badge variant="secondary"
											>{current.publication_year ?? 'Year unknown'}</Badge
										>
										{#if current.doi}<span
												class="max-w-full truncate font-mono text-xs text-muted-foreground"
												>{current.doi}</span
											>{/if}
									</div>
									<Badge
										variant={current.title_abstract_status === 'exclude'
											? 'destructive'
											: current.title_abstract_status === 'include'
												? 'default'
												: 'secondary'}
										>{statusLabel(current.title_abstract_status)}</Badge
									>
								</div>
								<h2
									class="max-w-4xl text-2xl leading-tight font-semibold tracking-tight sm:text-3xl"
								>
									{current.title ?? 'Untitled report'}
								</h2>
							</div>
							<div class="rounded-xl border bg-muted/20 p-4 sm:p-6">
								<div
									class="mb-3 flex items-center gap-2 text-[11px] font-semibold tracking-[0.12em] text-muted-foreground uppercase"
								>
									<span class="h-px w-5 bg-primary"></span> Abstract
								</div>
								<p
									class="text-sm leading-7 whitespace-pre-wrap text-foreground/90 sm:text-base"
								>
									{current.abstract_text ??
										'No abstract is available. Use Maybe when the available evidence is insufficient.'}
								</p>
							</div>
							<DecisionBar
								disabled={!protocol}
								pending={decisionMutation.isPending || undoMutation.isPending}
								{canUndo}
								onDecision={decide}
								onUndo={undo}
							/>
							<AiProposalReview
								{projectId}
								reportId={current.report_id}
								stage="title_abstract"
								protocolVersionId={protocol?.id}
								expectedRevision={current.revision}
							/>
						</article>
					{/if}
				</Card.Content>
			</Card.Root>

			<aside class="flex min-w-0 flex-col gap-6 lg:sticky lg:top-4 lg:self-start">
				<CriteriaPanel
					criteria={protocol?.criteria ?? []}
					protocolVersion={protocol?.version}
				/>
				{#if current}<ScreeningHistory items={historyItems} />{/if}
				<Card.Root size="sm" class="border-border/70">
					<Card.Header class="gap-2"
						><Card.Title class="text-sm">Keyboard shortcuts</Card.Title
						><Card.Description>Keep your hands on the queue.</Card.Description
						></Card.Header
					>
					<Card.Content class="flex flex-col gap-2 text-xs text-muted-foreground">
						<div class="flex items-center justify-between gap-2">
							<span><kbd>I</kbd> Include · <kbd>E</kbd> Exclude</span><span
								><kbd>M</kbd> Maybe</span
							>
						</div>
						<div><kbd>←</kbd>/<kbd>→</kbd> Previous / next · <kbd>U</kbd> Undo</div>
						<p class="border-t pt-2">
							Shortcuts pause while editing or using controls.
						</p>
					</Card.Content>
				</Card.Root>
			</aside>
		</div>
	{/if}
</div>

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
