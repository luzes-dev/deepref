<script lang="ts">
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import { onDestroy } from 'svelte';
	import * as Card from '$lib/components/ui/card';
	import * as Sheet from '$lib/components/ui/sheet';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Slider } from '$lib/components/ui/slider';
	import type { ArticleDto, GraphEdgeDto } from '$lib/api/generated/models';
	import { createGetProjectGraph } from '$lib/api/generated/articles/articles';
	import RotateCcwIcon from '@lucide/svelte/icons/rotate-ccw';
	import type SigmaType from 'sigma';

	let container = $state<HTMLDivElement | null>(null);
	let selected = $state<ArticleDto | null>(null);
	let search = $state('');
	let minInternal = $state(0);
	let renderer: SigmaType | undefined;
	let renderRun = 0;
	const projectId = $derived(page.params.projectId ?? '');
	const graphQuery = createGetProjectGraph(
		() => projectId,
		() => ({ query: { staleTime: 0 } })
	);
	const graphData = $derived(graphQuery.data?.data ?? { nodes: [], edges: [] });
	const visibleNodes = $derived(
		graphData.nodes.filter((node) => {
			const label = (node.title ?? node.doi).toLowerCase();
			return node.internal_citations >= minInternal && label.includes(search.toLowerCase());
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
			selected = nextGraph.getNodeAttributes(node) as ArticleDto;
		});
	}

	function clearGraph() {
		renderRun += 1;
		renderer?.kill();
		renderer = undefined;
	}

	function resetLayout() {
		if (container) void renderGraph(container, visibleNodes, graphData.edges);
	}

	$effect(() => {
		const target = container;
		const nodes = visibleNodes;
		const edges = graphData.edges;

		if (!target || nodes.length === 0) {
			clearGraph();
			return;
		}

		void renderGraph(target, nodes, edges);
	});

	onDestroy(() => renderer?.kill());
</script>

<div class="flex flex-col gap-6">
	<div class="flex items-center justify-between gap-4">
		<div>
			<h1 class="text-2xl font-semibold">Graph</h1>
			<p class="text-sm text-muted-foreground">
				Project-local citation network, directed from citing work to cited work.
			</p>
		</div>
		<Button variant="outline" onclick={resetLayout}
			><RotateCcwIcon data-icon="inline-start" />Reset layout</Button
		>
	</div>
	<Card.Root>
		<Card.Header>
			<Card.Title>Network controls</Card.Title>
			<Card.Description
				>{graphQuery.error?.message ??
					`${graphData.nodes.length} nodes and ${graphData.edges.length} edges`}</Card.Description
			>
		</Card.Header>
		<Card.Content class="grid gap-4 md:grid-cols-[1fr_280px]">
			<Input placeholder="Search node label" bind:value={search} />
			<div class="flex items-center gap-3">
				<Slider type="single" bind:value={minInternal} max={20} step={1} />
				<Badge variant="outline">Internal {minInternal}+</Badge>
			</div>
		</Card.Content>
	</Card.Root>
	<div class="grid gap-4 lg:grid-cols-[1fr_320px]">
		<div
			bind:this={container}
			class="relative h-[620px] overflow-hidden rounded-md border bg-muted"
		></div>
		<Card.Root>
			<Card.Header>
				<Card.Title>Matches</Card.Title>
				<Card.Description>Click a node or use search to inspect articles.</Card.Description>
			</Card.Header>
			<Card.Content class="flex max-h-[620px] flex-col gap-2 overflow-auto">
				{#each visibleNodes.slice(0, 30) as node (node.doi)}
					<button
						class="rounded-md border p-3 text-left hover:bg-muted"
						onclick={() => (selected = node)}
					>
						<div class="font-medium">{node.title ?? node.doi}</div>
						<div class="text-xs text-muted-foreground">{node.doi}</div>
					</button>
				{/each}
			</Card.Content>
		</Card.Root>
	</div>
</div>

<Sheet.Root open={!!selected} onOpenChange={(open) => !open && (selected = null)}>
	<Sheet.Content>
		<Sheet.Header>
			<Sheet.Title>{selected?.title ?? selected?.doi}</Sheet.Title>
			<Sheet.Description>{selected?.doi}</Sheet.Description>
		</Sheet.Header>
		{#if selected}
			<div class="mt-4 flex flex-col gap-3">
				<Badge>Rank {Number(selected.rank_score ?? 0).toFixed(2)}</Badge>
				<Badge variant="secondary">Internal {selected.internal_citations}</Badge>
				<Badge variant="outline">Total {selected.total_citations}</Badge>
				<Button href={resolve(`/projects/${projectId}/articles/${selected.doi_key}`)}
					>Open article</Button
				>
			</div>
		{/if}
	</Sheet.Content>
</Sheet.Root>
