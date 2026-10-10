<script lang="ts">
	import { resolve } from '$app/paths';
	import type { ResolvedPathname } from '$app/types';
	import { createGetDependencyStatus } from '#lib/api/generated/health/health.js';
	import {
		createGetProjectPrisma,
		createGetScreeningQueue
	} from '#lib/api/generated/review/review.js';
	import { statusVariant } from '#lib/api/helpers.js';
	import { createGetProjectReviewProtocol, isNotFound } from '#lib/features/protocol/api.js';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import { Skeleton } from '@deepref/ui/skeleton';
	import { StatePanel } from '@deepref/ui/layout';
	import ArrowRightIcon from '@lucide/svelte/icons/arrow-right';
	import CheckIcon from '@lucide/svelte/icons/check';
	import { createQuery } from '@tanstack/svelte-query';
	import { loadSynthesisProgress } from '#lib/features/studies/synthesis-progress.js';
	import PageTemplate from '#lib/shell/PageTemplate.svelte';
	import AiActivityGlance from '#lib/features/ai-autonomy/components/AiActivityGlance.svelte';
	import { useProjectWorkspaceContext } from '../context.svelte.js';
	import { reportLabel } from '../report-label';
	import {
		nextStage,
		reviewStages,
		type ProtocolSummary,
		type ReviewStage,
		type StageRoute
	} from '../review-progress';

	const workspace = useProjectWorkspaceContext();
	const projectId = $derived(workspace.selectedProjectId);
	// Scoped to this project, like the status badge in the workspace header.
	const dependenciesQuery = createGetDependencyStatus(
		() => ({ project_id: projectId ?? undefined }),
		() => ({ query: { staleTime: 5_000, refetchOnWindowFocus: 'always' } })
	);
	const prismaQuery = createGetProjectPrisma(
		() => projectId,
		() => ({ query: { enabled: Boolean(projectId), staleTime: 10_000 } })
	);
	const protocolQuery = createGetProjectReviewProtocol(
		() => projectId,
		() => ({ query: { enabled: Boolean(projectId), retry: false } })
	);

	// Only the progress split is needed: PRISMA reports unscreened and maybe as one number.
	const screeningQuery = createGetScreeningQueue(
		() => projectId,
		() => ({ status: 'maybe', limit: 1 }),
		() => ({ query: { enabled: Boolean(projectId), staleTime: 10_000 } })
	);
	const prisma = $derived(prismaQuery.data?.data);
	// Appraisal and extraction have no PRISMA counterpart, so completion is counted from their own lists.
	const synthesisQuery = createQuery(() => ({
		queryKey: [
			'overview-synthesis',
			projectId,
			prisma?.included_studies,
			prisma?.full_text_included
		],
		queryFn: () => loadSynthesisProgress(projectId),
		enabled: Boolean(projectId) && (prisma?.included_studies ?? 0) > 0,
		staleTime: 5_000
	}));
	const screeningProgress = $derived(screeningQuery.data?.data.progress);
	const protocol = $derived.by((): ProtocolSummary | undefined | null => {
		if (protocolQuery.data) {
			const value = protocolQuery.data.data;
			if (!value) return null;
			return {
				status: value.status,
				version: value.version,
				publishedAt: value.published_at
			};
		}
		if (protocolQuery.error && isNotFound(protocolQuery.error)) return null;
		return undefined;
	});
	const stages = $derived(
		prisma && protocol !== undefined
			? reviewStages(prisma, protocol, screeningProgress, synthesisQuery.data)
			: undefined
	);
	const next = $derived(stages ? nextStage(stages) : undefined);
	const noRecords = $derived(prisma?.identified_records === 0);

	const internalCitations = $derived(
		workspace.articles.reduce((sum, article) => sum + article.internal_citations, 0)
	);
	const leadingArticles = $derived(
		workspace.articles.toSorted((a, b) => b.rank_score - a.rank_score).slice(0, 6)
	);
	const dataError = $derived(workspace.articlesError ?? workspace.ingestionsError);
	const dependencyError = $derived(dependenciesQuery.error?.message);
	const hasEvidence = $derived(workspace.articles.length > 0 || workspace.ingestions.length > 0);
	const overviewState = $derived(
		dataError && !hasEvidence
			? 'error'
			: (workspace.articlesLoading || workspace.ingestionsLoading) && !hasEvidence
				? 'loading'
				: hasEvidence
					? 'populated'
					: 'empty'
	);
	const importsHref = $derived(resolve('/projects/[projectId]/discovery/imports', { projectId }));
	const articlesHref = $derived(resolve('/projects/[projectId]/articles', { projectId }));

	const stagePaths = {
		protocol: '/projects/[projectId]/protocol',
		imports: '/projects/[projectId]/discovery/imports',
		deduplication: '/projects/[projectId]/deduplication',
		'title-abstract': '/projects/[projectId]/screening/title-abstract',
		'full-text': '/projects/[projectId]/screening/full-text',
		studies: '/projects/[projectId]/studies',
		appraisal: '/projects/[projectId]/appraisal',
		extraction: '/projects/[projectId]/extraction'
	} as const satisfies Record<StageRoute, string>;

	const stageDestination: Record<StageRoute, string> = {
		protocol: 'protocol',
		imports: 'imports',
		deduplication: 'deduplication',
		'title-abstract': 'title & abstract',
		'full-text': 'full text',
		studies: 'studies',
		appraisal: 'appraisal',
		extraction: 'extraction'
	};

	function stageHref(stage: ReviewStage, withTodo = false): ResolvedPathname {
		const base = resolve(stagePaths[stage.id], { projectId });
		const search = withTodo ? stage.todo?.search : undefined;
		return (search ? `${base}?${new URLSearchParams(search)}` : base) as ResolvedPathname;
	}

	function articleHref(reportId: string): ResolvedPathname {
		return `${articlesHref}?report=${encodeURIComponent(reportId)}` as ResolvedPathname;
	}

	function ingestionHref(ingestionId: string): ResolvedPathname {
		return `${importsHref}?ingestion=${encodeURIComponent(ingestionId)}` as ResolvedPathname;
	}
</script>

<PageTemplate
	testId="overview-page"
	maxWidth="wide"
	role="region"
	aria-label="Overview content"
	data-overview-state={overviewState}
	containerClass="gap-8"
>
	<header class="flex flex-col gap-1">
		<h2 class="editorial-title text-xl">{workspace.project.name}</h2>
		{#if workspace.project.description}<p class="max-w-3xl text-sm text-muted-foreground">
				{workspace.project.description}
			</p>{/if}
	</header>

	{#if dependencyError}
		<div
			class="flex flex-wrap items-center justify-between gap-3"
			data-testid="overview-dependency-warning"
			role="status"
		>
			<p class="text-sm text-muted-foreground">
				Dependency status unavailable. Workspace evidence remains available. {dependencyError}
			</p>
			<Button
				variant="outline"
				size="sm"
				onclick={() => void dependenciesQuery.refetch()}
				disabled={dependenciesQuery.isFetching}>Refresh status</Button
			>
		</div>
	{/if}
	{#if dataError && hasEvidence}<p
			class="text-sm text-destructive"
			role="status"
			data-testid="overview-partial-data-warning"
		>
			Some evidence data is unavailable. Available data remains visible. {dataError}
		</p>{/if}

	{#if overviewState === 'error'}
		<div data-testid="overview-data-error">
			<StatePanel
				state="error"
				title="Evidence data unavailable"
				description={dataError ?? 'The evidence data could not be loaded.'}
			/>
		</div>
	{:else if overviewState === 'loading'}
		<div data-testid="overview-loading">
			<StatePanel
				state="loading"
				title="Gathering workspace evidence"
				description="Loading articles and import activity."
			/>
		</div>
	{:else}
		<div
			class="grid min-h-0 gap-x-14 gap-y-10 lg:grid-cols-[minmax(0,1fr)_minmax(16rem,22rem)]"
			data-testid="overview-populated"
		>
			<section aria-labelledby="overview-progress-title" class="flex min-w-0 flex-col gap-6">
				{#if next?.todo}
					<div
						class="flex flex-wrap items-center justify-between gap-x-6 gap-y-3 border-l-2 border-primary py-1 pl-4"
						data-testid="overview-next-step"
					>
						<div class="min-w-0">
							<p class="text-xs font-medium text-muted-foreground">
								Next step · {next.label}
							</p>
							<p class="text-lg font-semibold">{next.todo.label}</p>
						</div>
						<div class="flex flex-wrap items-center gap-2">
							<Button href={stageHref(next, true)}
								>Open {stageDestination[next.id]}<ArrowRightIcon
									data-icon="inline-end"
								/></Button
							>
							{#if noRecords && next.id !== 'imports'}
								<Button variant="outline" href={importsHref}>Import articles</Button
								>
							{/if}
						</div>
					</div>
				{:else if stages}
					{@const waitingStage = stages.find((stage) => stage.state !== 'done')}
					<p class="text-sm text-muted-foreground" data-testid="overview-next-step">
						{waitingStage
							? `${waitingStage.label}: ${waitingStage.summary}`
							: 'Every stage of the review is complete.'}
					</p>
				{/if}

				<div class="flex flex-col">
					<h3 id="overview-progress-title" class="sr-only">Review progress</h3>
					{#if stages}
						<ol class="flex flex-col" data-testid="overview-stages">
							{#each stages as stage (stage.id + stage.label)}
								<li>
									<a
										href={stageHref(stage, true)}
										class="group grid grid-cols-[1.25rem_minmax(0,1fr)] items-center gap-x-4 gap-y-1 rounded-md px-2 py-3 hover:bg-muted/60 focus-visible:outline-2 focus-visible:outline-ring sm:grid-cols-[1.25rem_10rem_minmax(0,1fr)_minmax(8rem,auto)]"
										data-stage-state={stage.state}
										aria-current={stage === next ? 'step' : undefined}
									>
										<span
											class={[
												'flex size-5 items-center justify-center rounded-full',
												stage.state === 'done' &&
													'bg-primary text-primary-foreground',
												stage.state === 'active' &&
													'border-2 border-primary',
												stage.state === 'waiting' && 'border border-border'
											]}
											aria-hidden="true"
										>
											{#if stage.state === 'done'}<CheckIcon
													class="size-3"
												/>{/if}
										</span>
										<span
											class={[
												'text-sm font-medium',
												stage.state === 'waiting' && 'text-muted-foreground'
											]}>{stage.label}</span
										>
										<span
											class="col-start-2 flex min-w-0 flex-col gap-1.5 sm:col-start-auto"
										>
											<span class="text-sm text-muted-foreground sm:truncate"
												>{stage.summary}</span
											>
											{#if stage.progress !== undefined && stage.state !== 'waiting'}
												<span
													class="h-1 w-full max-w-sm overflow-hidden rounded-full bg-muted"
													role="progressbar"
													aria-label="{stage.label} progress"
													aria-valuemin={0}
													aria-valuemax={100}
													aria-valuenow={Math.round(stage.progress * 100)}
												>
													<span
														class="block h-full rounded-full bg-primary"
														style:width="{stage.progress * 100}%"
													></span>
												</span>
											{/if}
										</span>
										{#if stage.todo}<span
												class="col-start-2 text-sm font-medium group-hover:text-primary sm:col-start-auto sm:text-right"
												>{stage.todo.label}</span
											>{/if}
									</a>
								</li>
							{/each}
						</ol>
					{:else if prismaQuery.error}
						<p class="text-sm text-muted-foreground" role="status">
							Review progress is unavailable. {prismaQuery.error.message}
						</p>
					{:else}
						<div class="flex flex-col gap-3" aria-label="Loading review progress">
							{#each { length: 6 }, index (index)}<Skeleton
									class="h-10 w-full"
								/>{/each}
						</div>
					{/if}
				</div>
			</section>

			<aside aria-label="Corpus" class="flex min-w-0 flex-col gap-8 text-sm">
				<section aria-labelledby="overview-corpus-title" class="flex flex-col gap-3">
					<div class="flex items-baseline justify-between gap-3">
						<h3 id="overview-corpus-title" class="font-semibold">Corpus</h3>
						<a
							class="inline-flex min-h-6 items-center text-muted-foreground hover:text-primary"
							href={articlesHref}>All articles</a
						>
					</div>
					<p class="text-muted-foreground tabular-nums">
						{workspace.articles.length.toLocaleString()}{workspace.articlesHasNextPage
							? '+'
							: ''} articles · {internalCitations.toLocaleString()} internal citations
					</p>
					<ol class="flex flex-col gap-2.5" aria-label="Most connected articles">
						{#each leadingArticles as article (article.report_id)}
							<li>
								<a
									class="flex min-h-6 items-baseline justify-between gap-3 hover:text-primary focus-visible:outline-2 focus-visible:outline-ring"
									href={articleHref(article.report_id)}
								>
									<span class="line-clamp-2">{reportLabel(article)}</span>
									<span
										class="shrink-0 text-xs text-muted-foreground tabular-nums"
										>{article.issued_year ?? '—'}</span
									>
								</a>
							</li>
						{/each}
					</ol>
				</section>

				<section
					aria-labelledby="overview-imports-title"
					class="flex flex-col gap-3"
					data-testid="overview-ingestions"
				>
					<div class="flex items-baseline justify-between gap-3">
						<h3 id="overview-imports-title" class="font-semibold">Imports</h3>
						<a
							class="inline-flex min-h-6 items-center text-muted-foreground hover:text-primary"
							href={importsHref}>Add articles</a
						>
					</div>
					{#each workspace.ingestions.slice(0, 4) as ingestion (ingestion.id)}<a
							href={ingestionHref(ingestion.id)}
							class="flex min-h-6 flex-wrap items-center gap-x-3 gap-y-1 hover:text-primary"
							><Badge size="sm" variant={statusVariant(ingestion.status)}
								>{ingestion.status}</Badge
							><span class="tabular-nums"
								>{ingestion.fetched_count} fetched · {ingestion.seed_count} seeds</span
							><span class="text-xs text-muted-foreground"
								>{new Date(ingestion.created_at).toLocaleDateString()}</span
							></a
						>{:else}<p class="text-muted-foreground">
							No imports yet. Runs you start from Imports will appear here.
						</p>{/each}
				</section>

				{#if projectId}<AiActivityGlance {projectId} />{/if}
			</aside>
		</div>
	{/if}
</PageTemplate>
