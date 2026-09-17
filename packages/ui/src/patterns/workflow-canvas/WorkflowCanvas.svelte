<script lang="ts">
	import {
		SvelteFlow,
		Background,
		MiniMap,
		type Node,
		type Edge,
		type NodeTypes,
		type Connection,
		addEdge,
	} from "@xyflow/svelte";
	import "@xyflow/svelte/dist/style.css";

	import TriggerNode from "./nodes/TriggerNode.svelte";
	import ActionNode from "./nodes/ActionNode.svelte";
	import AgentNode from "./nodes/AgentNode.svelte";
	import ConditionNode from "./nodes/ConditionNode.svelte";
	import HumanGateNode from "./nodes/HumanGateNode.svelte";
	import CanvasControls from "./CanvasControls.svelte";
	import {
		WORKFLOW_FIT_VIEW_OPTIONS,
		WORKFLOW_ZOOM_LIMITS,
	} from "./viewport-options";

	let {
		nodes = $bindable<Node[]>([]),
		edges = $bindable<Edge[]>([]),
		selectedNodeId = $bindable<string | null>(null),
		onNodeClick,
		onRunWorkflow,
		onShowHistory,
		onAutoLayout,
		fitView = true,
		readOnly = false,
	}: {
		nodes: Node[];
		edges: Edge[];
		selectedNodeId?: string | null;
		onNodeClick?: (node: Node) => void;
		onRunWorkflow?: () => void;
		onShowHistory?: () => void;
		onAutoLayout?: () => void;
		fitView?: boolean;
		readOnly?: boolean;
	} = $props();

	const nodeTypes: NodeTypes = {
		trigger: TriggerNode,
		action: ActionNode,
		agent: AgentNode,
		condition: ConditionNode,
		human_gate: HumanGateNode,
	};

	function handleConnect(connection: Connection) {
		if (readOnly) return;
		edges = addEdge(
			{
				...connection,
				animated: true,
				style: "stroke: var(--primary); stroke-width: 2px;",
			},
			edges,
		);
	}

	function handleDragOver(event: DragEvent): void {
		event.preventDefault();
		if (event.dataTransfer) event.dataTransfer.dropEffect = "move";
	}

	function handleDrop(event: DragEvent): void {
		event.preventDefault();
		const raw = event.dataTransfer?.getData("application/deepref-node");
		if (!raw) return;
		try {
			const item = JSON.parse(raw);
			const newNode: Node = {
				id: `${item.type ?? "action"}-${Date.now()}`,
				type: item.type ?? "action",
				position: {
					x: 250 + Math.floor(Math.random() * 80),
					y: 120 + Math.floor(Math.random() * 80),
				},
				data: item.defaultData ?? item.data ?? { label: "New Node" },
			};
			nodes = [...nodes, newNode];
			selectedNodeId = newNode.id;
			onNodeClick?.(newNode);
		} catch {
			// ignore
		}
	}
</script>

<div
	role="region"
	aria-label="Workflow canvas"
	aria-describedby="workflow-canvas-hint"
	class="deepref-workflow relative h-full w-full overflow-hidden rounded-lg border border-border bg-card shadow-none select-none"
	ondragover={handleDragOver}
	ondrop={handleDrop}
>
	<SvelteFlow
		bind:nodes
		bind:edges
		{nodeTypes}
		{fitView}
		fitViewOptions={WORKFLOW_FIT_VIEW_OPTIONS}
		minZoom={WORKFLOW_ZOOM_LIMITS.minZoom}
		maxZoom={WORKFLOW_ZOOM_LIMITS.maxZoom}
		nodesDraggable={!readOnly}
		nodesConnectable={!readOnly}
		elementsSelectable={true}
		onconnect={handleConnect}
		onnodeclick={({ node }) => {
			selectedNodeId = node.id;
			onNodeClick?.(node);
		}}
		defaultEdgeOptions={{
			animated: true,
			style: "stroke: var(--primary); stroke-width: 2px;",
		}}
	>
		<!-- Background dot matrix -->
		<Background gap={22} size={1.2} patternColor="var(--border)" />

		<!-- Bottom Floating Zoom Bar + Vertical Side Rail -->
		<CanvasControls {onRunWorkflow} {onShowHistory} {onAutoLayout} />

		<!-- MiniMap docked in bottom-right corner -->
		<MiniMap
			position="bottom-right"
			nodeColor={(n) => {
				if (n.type === "trigger") return "var(--chart-1)";
				if (n.type === "action") return "var(--chart-4)";
				if (n.type === "agent") return "var(--chart-5)";
				if (n.type === "condition") return "var(--chart-2)";
				if (n.type === "human_gate") return "var(--chart-3)";
				return "var(--muted-foreground)";
			}}
			class="!m-4 hidden !rounded-lg !border !border-border !bg-card !shadow-none md:block"
		/>
	</SvelteFlow>
	<p id="workflow-canvas-hint" class="sr-only">
		Drag to pan and pinch to zoom. Use the controls to change the zoom or
		fit the workflow in view.
	</p>
	<p
		aria-hidden="true"
		class="pointer-events-none absolute top-3 left-3 rounded-md border border-border-subtle bg-card/90 px-2 py-1 text-[11px] text-muted-foreground backdrop-blur-sm md:hidden"
	>
		Drag to pan · pinch to zoom
	</p>
</div>

<style>
	.deepref-workflow :global(.svelte-flow) {
		--xy-edge-label-background-color: var(--card);
		--xy-edge-label-color: var(--foreground);
		background-color: transparent !important;
	}
	.deepref-workflow :global(.svelte-flow__edge-path) {
		stroke: var(--primary) !important;
		stroke-width: 2px !important;
	}
	.deepref-workflow :global(.svelte-flow__handle) {
		width: 10px !important;
		height: 10px !important;
		background: var(--primary) !important;
		border: 2px solid var(--card) !important;
		transition:
			transform 0.15s ease,
			box-shadow 0.15s ease;
	}
	.deepref-workflow :global(.svelte-flow__handle:hover) {
		transform: scale(1.4);
		box-shadow: 0 0 0 2px var(--ring);
	}
</style>
