<script lang="ts">
	import type { GraphNodeDto, ProjectGraphDto } from '$lib/api/generated/models';
	import { createGetProjectGraph } from '$lib/api/generated/reports/reports';
	import GraphDegradedState from '$lib/components/GraphDegradedState.svelte';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Checkbox } from '$lib/components/ui/checkbox';
	import * as Empty from '$lib/components/ui/empty';
	import * as InputGroup from '$lib/components/ui/input-group';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { Slider } from '$lib/components/ui/slider';
	import { Spinner } from '$lib/components/ui/spinner';
	import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
	import SearchIcon from '@lucide/svelte/icons/search';
	import { onCleanup, watch } from 'runed';
	import { useProjectWorkspaceContext, type GraphOverlayField } from './context.svelte.js';
	import { overlaySummary } from './graph-overlays';
	import {
		createProjectGraphRenderer,
		type ProjectGraphRenderModel
	} from './project-graph-renderer';
	import { activeProjectQuery, createActiveProjectProjection } from './project-queries.svelte';
	import { reportSearchText } from './report-label';

	const FILTER_UPDATE_DELAY_MS = 150;
	const overlayLabels: ReadonlyArray<{ field: GraphOverlayField; label: string }> = [
		{ field: 'metrics', label: 'Metrics' },
		{ field: 'screening', label: 'Screening' },
		{ field: 'study', label: 'Study' },
		{ field: 'appraisal', label: 'Appraisal' },
		{ field: 'provenance', label: 'Provenance' }
	];

	const workspace = useProjectWorkspaceContext();
	let container = $state<HTMLDivElement | null>(null);
	let graphRendering = $state(false);
	let renderVersion = 0;
	const enabled = $derived(workspace.view === 'graph');

	const graphQuery = createGetProjectGraph(
		() => workspace.project.id,
		() => ({ fields: workspace.graphFilters.fields.join(',') }),
		() => activeProjectQuery(workspace.project.id, enabled)
	);
	const projectionQuery = createActiveProjectProjection(
		() => workspace.project.id,
		() => enabled
	);
	const graphData = $derived<ProjectGraphDto>(
		graphQuery.data?.data ?? {
			nodes: [],
			edges: [],
			projection: { revision: 0, lag: 0 },
			truncated: false
		}
	);

	function internalCitations(node: GraphNodeDto): number {
		return node.metrics?.internal_citations ?? 0;
	}

	function overlayEnabled(field: GraphOverlayField): boolean {
		return workspace.graphFilters.fields.includes(field);
	}

	const visibleNodes = $derived(
		graphData.nodes.filter((node) => {
			const term = workspace.graphFilters.search.toLowerCase();
			return (
				(!overlayEnabled('metrics') ||
					internalCitations(node) >= workspace.graphFilters.minInternal) &&
				reportSearchText(node).includes(term)
			);
		})
	);
	const selectedGraphNode = $derived(
		graphData.nodes.find((node) => node.report_id === workspace.selectedArticle)
	);
	const visibleNodeIds = $derived(new Set(visibleNodes.map((node) => node.report_id)));

	function renderModel(): ProjectGraphRenderModel {
		return {
			nodes: graphData.nodes,
			edges: graphData.edges,
			visibleNodeIds,
			selectedArticle: workspace.selectedArticle,
			colorBy: workspace.graphFilters.colorBy,
			fields: workspace.graphFilters.fields
		};
	}

	const graphRenderer = createProjectGraphRenderer({
		onSelect: (reportId) => workspace.openArticle(reportId),
		onClear: () => workspace.clearArticle()
	});

	function graphContainerAttachment(element: Element) {
		if (!(element instanceof HTMLDivElement)) {
			throw new TypeError('The graph renderer requires an HTMLDivElement');
		}
		container = element;
		graphRenderer.mount(element);
		return () => {
			if (container === element) container = null;
		};
	}

	async function renderCurrentGraph(options: { resetCamera?: boolean } = {}) {
		const version = ++renderVersion;
		graphRendering = true;
		try {
			await graphRenderer.update(renderModel(), options);
		} finally {
			if (version === renderVersion) graphRendering = false;
		}
	}

	function resetLayout() {
		if (container && enabled && visibleNodes.length > 0) {
			void renderCurrentGraph({ resetCamera: true });
		}
	}

	watch(
		[() => enabled, () => container, () => graphData.nodes, () => graphData.edges],
		([active, target, nodes]) => {
			if (!active || !target || nodes.length === 0) {
				renderVersion += 1;
				graphRenderer.clear();
				graphRendering = false;
				return;
			}
			void renderCurrentGraph();
		}
	);

	watch([() => enabled, () => visibleNodeIds], ([active, nextVisibleNodeIds]) => {
		if (!active) return;
		const timeout = setTimeout(
			() => graphRenderer.setVisibleNodes(nextVisibleNodeIds),
			FILTER_UPDATE_DELAY_MS
		);
		return () => clearTimeout(timeout);
	});

	watch([() => enabled, () => workspace.selectedArticle], ([active, selectedArticle]) => {
		if (active) graphRenderer.setSelection(selectedArticle);
	});

	watch(
		[
			() => enabled,
			() => workspace.graphFilters.colorBy,
			() => workspace.graphFilters.fields.join(',')
		],
		([active]) => {
			if (!active) return;
			graphRenderer.refreshAppearance({
				colorBy: workspace.graphFilters.colorBy,
				fields: workspace.graphFilters.fields
			});
		}
	);

	onCleanup(() => graphRenderer.destroy());
</script>

<div class="flex h-full min-h-0 flex-col gap-4 p-4">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold">Graph</h1>
			<p class="text-sm text-muted-foreground">
				Project-local citation network, directed from citing work to cited work.
			</p>
		</div>
		{#if graphQuery.data}
			<div class="flex flex-wrap gap-2" data-testid="projection-metadata">
				<Badge variant="secondary"
					>Projection revision {graphData.projection.revision}</Badge
				>
				<Badge variant="outline">Lag {graphData.projection.lag}</Badge>
				{#if graphData.projection.last_success_at}
					<Badge variant="outline">
						Projected {new Date(graphData.projection.last_success_at).toLocaleString()}
					</Badge>
				{/if}
				{#if graphData.truncated}<Badge variant="secondary">Bounded result</Badge>{/if}
			</div>
		{/if}
	</div>

	<div class="grid gap-3 md:grid-cols-[1fr_280px_auto]">
		<InputGroup.Root>
			<InputGroup.Input
				placeholder="Search node label"
				bind:value={workspace.graphFilters.search}
			/>
			<InputGroup.Addon><SearchIcon /></InputGroup.Addon>
		</InputGroup.Root>
		<div class="flex items-center gap-3">
			<Slider
				type="single"
				bind:value={workspace.graphFilters.minInternal}
				max={20}
				step={1}
				disabled={!overlayEnabled('metrics')}
			/>
			<Badge variant="outline">
				{overlayEnabled('metrics')
					? `Internal ${workspace.graphFilters.minInternal}+`
					: 'Internal filter unavailable'}
			</Badge>
		</div>
		<Badge variant="secondary"
			>{graphData.nodes.length} nodes and {graphData.edges.length} edges</Badge
		>
	</div>
	<fieldset class="flex flex-wrap items-center gap-3" data-testid="graph-overlay-filters">
		<legend class="mr-1 text-sm font-medium">Visual overlays</legend>
		{#each overlayLabels as { field, label } (field)}
			<label class="flex items-center gap-2 text-sm text-muted-foreground">
				<Checkbox
					checked={overlayEnabled(field)}
					onCheckedChange={(checked) =>
						workspace.graphFilters.setField(field, checked === true)}
					aria-label={`Load ${label.toLowerCase()} overlay`}
					data-testid={`graph-overlay-${field}`}
				/>
				{label}
			</label>
		{/each}
		<label class="ml-auto flex items-center gap-2 text-sm">
			<span class="text-muted-foreground">Color by</span>
			<select
				aria-label="Color graph by"
				class="h-8 rounded-md border bg-background px-2"
				value={workspace.graphFilters.colorBy}
				onchange={(event) =>
					(workspace.graphFilters.colorBy = (event.currentTarget as HTMLSelectElement)
						.value as GraphOverlayField)}
			>
				{#each overlayLabels as { field, label } (field)}
					<option value={field} disabled={!overlayEnabled(field)}>{label}</option>
				{/each}
			</select>
		</label>
	</fieldset>
	<div class="grid gap-3 md:grid-cols-2" data-testid="graph-overlay-legend">
		{#if !overlayEnabled(workspace.graphFilters.colorBy)}
			<div class="rounded-md border p-3 text-sm text-muted-foreground">
				Color-by overlay is not loaded. Select it above or enable its field.
			</div>
		{:else if workspace.graphFilters.colorBy === 'screening'}
			<div class="rounded-md border p-3 text-sm">
				<div class="font-medium">Screening overlay</div>
				<div class="mt-2 flex flex-wrap gap-3 text-muted-foreground">
					<span class="screening-include">● include</span>
					<span class="screening-exclude">● exclude</span>
					<span class="screening-pending">● pending / unscreened</span>
				</div>
			</div>
		{:else if workspace.graphFilters.colorBy === 'metrics'}
			<div class="rounded-md border p-3 text-sm">
				<div class="font-medium">Metrics overlay</div>
				<div class="mt-2 flex flex-wrap gap-3 text-muted-foreground">
					<span class="metrics-cited">● internally cited</span>
					<span class="metrics-uncited">● no internal citations</span>
				</div>
				<p class="mt-2 text-xs text-muted-foreground">
					Rank and citation counts are available when a node is selected.
				</p>
			</div>
		{:else}
			<div class="rounded-md border p-3 text-sm">
				<div class="font-medium">Evidence overlays</div>
				<div class="mt-2 flex flex-wrap gap-3 text-muted-foreground">
					{#if workspace.graphFilters.colorBy === 'study'}<span class="evidence-grouped"
							>● grouped</span
						><span class="evidence-ungrouped">● ungrouped</span>{/if}
					{#if workspace.graphFilters.colorBy === 'appraisal'}<span
							class="evidence-appraised">● appraised</span
						><span class="evidence-not-appraised">● not appraised</span>{/if}
					{#if workspace.graphFilters.colorBy === 'provenance'}<span
							class="evidence-acquired">● acquired</span
						><span class="evidence-no-source">● no source</span>{/if}
				</div>
			</div>
		{/if}
	</div>
	{#if selectedGraphNode}
		<aside class="rounded-md border bg-muted/20 p-3" aria-label="Selected node overlay summary">
			<div class="text-sm font-medium">Selected node overlays</div>
			<div class="mt-2 grid gap-1 text-sm text-muted-foreground sm:grid-cols-2">
				{#each overlaySummary(selectedGraphNode, workspace.graphFilters.fields) as summary (summary)}
					<div>{summary}</div>
				{/each}
			</div>
		</aside>
	{/if}
	{#if graphQuery.error}
		<GraphDegradedState
			error={graphQuery.error}
			projection={projectionQuery.data?.data}
			onRetry={() => void graphQuery.refetch()}
		/>
	{:else if graphQuery.isPending && enabled}
		<Skeleton class="min-h-[520px] flex-1" />
	{:else if graphData.nodes.length === 0 || visibleNodes.length === 0}
		<Empty.Root class="min-h-[520px] flex-1">
			<Empty.Header>
				<Empty.Title>Empty graph</Empty.Title>
				<Empty.Description
					>No graph nodes match this project and filter set.</Empty.Description
				>
			</Empty.Header>
		</Empty.Root>
	{:else}
		<div class="min-h-0 flex-1">
			<div
				class="graph-frame relative h-full overflow-hidden rounded-md border"
				aria-busy={graphRendering}
			>
				<div {@attach graphContainerAttachment} class="absolute inset-0"></div>
				{#if graphRendering}
					<div
						class="absolute inset-0 z-20 flex items-center justify-center bg-background/70 backdrop-blur-[1px]"
					>
						<div
							class="flex items-center gap-2 rounded-md border bg-background px-3 py-2 text-sm shadow-sm"
						>
							<Spinner class="size-4" />
							<span>Creating graph layout</span>
						</div>
					</div>
				{/if}
				<Button
					variant="ghost"
					size="icon"
					class="absolute top-3 right-3 z-10 bg-background/80"
					onclick={resetLayout}
					disabled={graphRendering}
					aria-label="Reset graph layout"
				>
					<RotateCcwIcon data-icon />
				</Button>
			</div>
		</div>
	{/if}
</div>

<style>
	.graph-frame {
		--graph-bg: #f8fafc;
		--graph-node-default: #64748b;
		--graph-node-cited: #2563eb;
		--graph-node-selected: #d97706;
		--graph-node-focused: #0891b2;
		--graph-node-dimmed: #cbd5e1;
		--graph-edge-default: #94a3b8;
		--graph-edge-focused: #0284c7;
		--graph-edge-dimmed: #dbe4ee;
		--graph-label: #0f172a;
		--graph-label-muted: #475569;
		--graph-label-halo: #f8fafc;
		--graph-hover-background: #ffffff;
		background: var(--graph-bg);
		cursor: grab;
	}

	:global(.dark) .graph-frame {
		--graph-bg: #020617;
		--graph-node-default: #94a3b8;
		--graph-node-cited: #60a5fa;
		--graph-node-selected: #f59e0b;
		--graph-node-focused: #22d3ee;
		--graph-node-dimmed: #334155;
		--graph-edge-default: #475569;
		--graph-edge-focused: #38bdf8;
		--graph-edge-dimmed: #1e293b;
		--graph-label: #e2e8f0;
		--graph-label-muted: #94a3b8;
		--graph-label-halo: #020617;
		--graph-hover-background: #0f172a;
	}

	.graph-frame:global(.graph-dragging) {
		cursor: grabbing;
	}

	.graph-frame :global(canvas) {
		cursor: inherit;
	}

	.screening-include,
	.evidence-appraised {
		color: #2563eb;
	}

	.screening-exclude {
		color: #d97706;
	}

	.screening-pending {
		color: #0891b2;
	}

	.metrics-cited {
		color: #2563eb;
	}

	.metrics-uncited {
		color: #64748b;
	}

	.evidence-grouped,
	.evidence-acquired {
		color: #0891b2;
	}

	.evidence-ungrouped,
	.evidence-no-source {
		color: #cbd5e1;
	}

	.evidence-not-appraised {
		color: #64748b;
	}
</style>
