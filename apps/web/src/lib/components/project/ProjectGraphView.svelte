<script lang="ts">
	import * as Alert from '$lib/components/ui/alert';
	import * as Empty from '$lib/components/ui/empty';
	import * as InputGroup from '$lib/components/ui/input-group';
	import * as Sheet from '$lib/components/ui/sheet';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { Slider } from '$lib/components/ui/slider';
	import type { ArticleDto, GraphEdgeDto } from '$lib/api/generated/models';
	import { createGetProjectGraph } from '$lib/api/generated/articles/articles';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
	import SearchIcon from '@lucide/svelte/icons/search';
	import { onCleanup } from 'runed';
	import { useProjectWorkspaceContext } from './context.svelte.js';
	import type SigmaType from 'sigma';

	const workspace = useProjectWorkspaceContext();
	let container = $state<HTMLDivElement | null>(null);
	let renderer: SigmaType | undefined;
	let renderRun = 0;
	const enabled = $derived(workspace.view === 'graph');

	const graphQuery = createGetProjectGraph(
		() => workspace.project.id,
		() => ({ query: { enabled: Boolean(workspace.project.id && enabled), staleTime: 0 } })
	);
	const graphData = $derived(graphQuery.data?.data ?? { nodes: [], edges: [] });
	const visibleNodes = $derived(
		graphData.nodes.filter((node) => {
			const label = (node.title ?? node.doi).toLowerCase();
			const term = workspace.graphFilters.search.toLowerCase();
			return (
				node.internal_citations >= workspace.graphFilters.minInternal &&
				(label.includes(term) || node.doi.toLowerCase().includes(term))
			);
		})
	);

	async function renderGraph(target: HTMLDivElement, nodes: ArticleDto[], edges: GraphEdgeDto[]) {
		const run = ++renderRun;
		const [{ default: Graph }, { default: Sigma }, forceAtlas2] = await Promise.all([
			import('graphology'),
			import('sigma'),
			import('graphology-layout-forceatlas2')
		]);
		if (run !== renderRun) return;

		const nextGraph = new Graph();
		for (const [index, node] of nodes.entries()) {
			nextGraph.addNode(node.doi, {
				...node,
				label: node.title ?? node.doi,
				type: 'circle',
				x: Math.cos(index) * 10,
				y: Math.sin(index) * 10,
				size: 4 + Math.max(1, node.rank_score * 12),
				color: node.internal_citations > 0 ? '#2563eb' : '#64748b'
			});
		}
		for (const edge of edges) {
			if (
				nextGraph.hasNode(edge.source) &&
				nextGraph.hasNode(edge.target) &&
				edge.source !== edge.target &&
				!nextGraph.hasEdge(edge.source, edge.target)
			) {
				nextGraph.addDirectedEdge(edge.source, edge.target, { size: 1, color: '#94a3b8' });
			}
		}
		if (nextGraph.order > 2) {
			forceAtlas2.default.assign(nextGraph, {
				iterations: 80,
				settings: forceAtlas2.default.inferSettings(nextGraph)
			});
		}
		renderer?.kill();
		renderer = new Sigma(nextGraph, target);
		renderer.on('clickNode', ({ node }: { node: string }) => {
			workspace.graphFilters.selected = nextGraph.getNodeAttributes(node) as ArticleDto;
		});
	}

	function clearGraph() {
		renderRun += 1;
		renderer?.kill();
		renderer = undefined;
	}

	function resetLayout() {
		if (container && enabled && visibleNodes.length > 0) {
			void renderGraph(container, visibleNodes, graphData.edges);
		}
	}

	$effect(() => {
		const target = container;
		const nodes = visibleNodes;
		const edges = graphData.edges;
		const active = enabled;

		if (!active || !target || nodes.length === 0) {
			clearGraph();
			return;
		}

		void renderGraph(target, nodes, edges);
	});

	onCleanup(() => clearGraph());
</script>

<div class="flex h-full min-h-0 flex-col gap-4 p-4">
	<div class="flex flex-wrap items-center justify-between gap-3">
		<div>
			<h1 class="text-2xl font-semibold">Graph</h1>
			<p class="text-sm text-muted-foreground">
				Project-local citation network, directed from citing work to cited work.
			</p>
		</div>
		<Button variant="outline" onclick={resetLayout}>
			<RotateCcwIcon data-icon="inline-start" />Reset layout
		</Button>
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
			/>
			<Badge variant="outline">Internal {workspace.graphFilters.minInternal}+</Badge>
		</div>
		<Badge variant="secondary"
			>{graphData.nodes.length} nodes and {graphData.edges.length} edges</Badge
		>
	</div>

	{#if graphQuery.error}
		<Alert.Root variant="destructive">
			<CircleAlertIcon />
			<Alert.Title>Graph unavailable</Alert.Title>
			<Alert.Description>{graphQuery.error.message}</Alert.Description>
		</Alert.Root>
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
		<div class="grid min-h-0 flex-1 gap-4 lg:grid-cols-[1fr_300px]">
			<div
				bind:this={container}
				class="relative min-h-[520px] overflow-hidden rounded-md border bg-muted"
			></div>
			<section class="hidden min-h-0 rounded-md border lg:block">
				<div class="border-b p-4">
					<h2 class="font-medium">Matches</h2>
					<p class="text-sm text-muted-foreground">
						Click a node or list item to inspect it.
					</p>
				</div>
				<div class="flex max-h-[calc(100vh-18rem)] flex-col gap-2 overflow-auto p-3">
					{#each visibleNodes.slice(0, 30) as node (node.doi)}
						<button
							class="rounded-md border p-3 text-left hover:bg-muted"
							onclick={() => (workspace.graphFilters.selected = node)}
						>
							<div class="truncate font-medium">{node.title ?? node.doi}</div>
							<div class="truncate text-xs text-muted-foreground">{node.doi}</div>
						</button>
					{/each}
				</div>
			</section>
		</div>
	{/if}
</div>

<Sheet.Root
	open={!!workspace.graphFilters.selected}
	onOpenChange={(open) => !open && workspace.resetGraphSelection()}
>
	<Sheet.Content>
		<Sheet.Header>
			<Sheet.Title>
				{workspace.graphFilters.selected?.title ??
					workspace.graphFilters.selected?.doi ??
					'Article'}
			</Sheet.Title>
			<Sheet.Description>{workspace.graphFilters.selected?.doi}</Sheet.Description>
		</Sheet.Header>
		{#if workspace.graphFilters.selected}
			<div class="mt-4 flex flex-col gap-3">
				<div class="flex flex-wrap gap-2">
					<Badge
						>Rank {Number(workspace.graphFilters.selected.rank_score ?? 0).toFixed(
							2
						)}</Badge
					>
					<Badge variant="secondary"
						>Internal {workspace.graphFilters.selected.internal_citations}</Badge
					>
					<Badge variant="outline"
						>Total {workspace.graphFilters.selected.total_citations}</Badge
					>
				</div>
				<Button
					onclick={() =>
						workspace.openArticle(workspace.graphFilters.selected?.doi_key ?? '')}
					>Open article</Button
				>
			</div>
		{/if}
	</Sheet.Content>
</Sheet.Root>
