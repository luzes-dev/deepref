<script lang="ts">
	import {
		SvelteFlow,
		Background,
		BackgroundVariant,
		MiniMap,
		MarkerType,
		type Node,
		type Edge,
		type NodeTypes,
		type Connection,
	} from "@xyflow/svelte";
	import "@xyflow/svelte/dist/style.css";
	import { setContext, type Component } from "svelte";
	import BuilderNode from "./BuilderNode.svelte";
	import BuilderNoteNode from "./BuilderNoteNode.svelte";
	import BuilderApiBridge from "./BuilderApiBridge.svelte";
	import CanvasControls from "./CanvasControls.svelte";
	import { BUILDER_CONTEXT, type BuilderContext } from "./builder-context.js";
	import {
		BUILDER_FIT_VIEW_OPTIONS,
		BUILDER_ZOOM_LIMITS,
	} from "./viewport-options";
	import type {
		BuilderCanvasApi,
		BuilderNodeData,
		BuilderTone,
	} from "./types.js";

	interface ConnectDrop {
		nodeId: string;
		portId: string;
		clientX: number;
		clientY: number;
	}
	interface ConnectRefused {
		from: { nodeId: string; portId: string };
		to: { nodeId: string; portId: string };
		clientX: number;
		clientY: number;
	}
	interface PortRef {
		nodeId: string;
		portId: string;
	}

	let {
		nodes = $bindable<Node[]>([]),
		edges = $bindable<Edge[]>([]),
		readOnly = false,
		snapSize = 20,
		fitOnLoad = false,
		iconFor,
		onready,
		onconnect,
		isValidConnection,
		onconnectrefused,
		onconnectdrop,
		onarm,
		onportadd,
		onnodeselect,
		onpaneclick,
		onnodecontextmenu,
		onpanecontextmenu,
		onedgecontextmenu,
		onchange,
		ondeletenodes,
		ondeleteedges,
	}: {
		nodes: Node[];
		edges: Edge[];
		readOnly?: boolean;
		snapSize?: number;
		/**
		 * Fit the whole graph when the canvas opens. Pass it only when the graph
		 * is not empty, otherwise the first block added later would be fitted too.
		 */
		fitOnLoad?: boolean;
		/** Icon for a block, by the node's `iconKey`. */
		iconFor?: (key: string) => Component<{ class?: string }> | undefined;
		onready?: (api: BuilderCanvasApi) => void;
		onconnect?: (connection: Connection) => void;
		isValidConnection?: (connection: Connection) => boolean;
		onconnectrefused?: (event: ConnectRefused) => void;
		onconnectdrop?: (event: ConnectDrop) => void;
		/**
		 * A port was tapped to start a connection (`from`), or the connection
		 * ended or was cancelled (`null`). Tap-to-connect works without dragging.
		 */
		onarm?: (from: PortRef | null) => void;
		/** The "+" button next to a free output was pressed. */
		onportadd?: (nodeId: string, portId: string, anchor: HTMLElement) => void;
		onnodeselect?: (nodeId: string | null) => void;
		onpaneclick?: () => void;
		onnodecontextmenu?: (nodeId: string, event: MouseEvent) => void;
		onpanecontextmenu?: (event: MouseEvent) => void;
		onedgecontextmenu?: (edgeId: string, event: MouseEvent) => void;
		/** Called after the user moved something on the canvas. */
		onchange?: () => void;
		ondeletenodes?: (ids: string[]) => void;
		ondeleteedges?: (ids: string[]) => void;
	} = $props();

	const nodeTypes: NodeTypes = { block: BuilderNode, note: BuilderNoteNode };

	setContext<BuilderContext>(BUILDER_CONTEXT, {
		iconFor: (key) => iconFor?.(key),
		addFrom: (nodeId, portId, anchor) => onportadd?.(nodeId, portId, anchor),
	});

	const TONE_FILL: Record<BuilderTone, string> = {
		trigger: "var(--success)",
		data: "var(--info)",
		action: "var(--primary)",
		ai: "var(--chart-5)",
		logic: "var(--chart-2)",
		integration: "var(--chart-4)",
		note: "var(--warning)",
	};
	function minimapFill(node: Node): string {
		const tone = (node.data as Partial<BuilderNodeData>).tone;
		return (tone && TONE_FILL[tone]) || "var(--muted-foreground)";
	}
</script>

<div
	role="region"
	aria-label="Automation canvas"
	class="deepref-builder relative h-full w-full overflow-hidden bg-background"
	data-testid="builder-canvas"
>
	<SvelteFlow
		bind:nodes
		bind:edges
		{nodeTypes}
		fitView={fitOnLoad}
		fitViewOptions={BUILDER_FIT_VIEW_OPTIONS}
		minZoom={BUILDER_ZOOM_LIMITS.minZoom}
		maxZoom={BUILDER_ZOOM_LIMITS.maxZoom}
		snapGrid={[snapSize, snapSize]}
		nodesDraggable={!readOnly}
		nodesConnectable={!readOnly}
		elementsSelectable={true}
		deleteKey={readOnly ? null : ["Delete", "Backspace"]}
		panActivationKey="Space"
		selectionKey="Shift"
		multiSelectionKey={["Meta", "Control"]}
		selectionOnDrag={false}
		connectionRadius={28}
		attributionPosition="bottom-center"
		defaultEdgeOptions={{
			markerEnd: { type: MarkerType.ArrowClosed, width: 16, height: 16 },
		}}
		isValidConnection={(connection) =>
			isValidConnection
				? isValidConnection(connection as Connection)
				: true}
		onconnect={(connection) => onconnect?.(connection)}
		onclickconnectstart={(_event, handle) => {
			if (handle.nodeId && handle.handleType === "source") {
				onarm?.({ nodeId: handle.nodeId, portId: handle.handleId ?? "" });
			}
		}}
		onclickconnectend={() => onarm?.(null)}
		onconnectend={(_event, state) => {
			const from = state.fromNode && state.fromHandle;
			if (!from || state.fromHandle?.type !== "source") return;
			const pointer = _event instanceof MouseEvent ? _event : null;
			const clientX = pointer?.clientX ?? 0;
			const clientY = pointer?.clientY ?? 0;
			if (state.toNode && state.toHandle && !state.isValid) {
				onconnectrefused?.({
					from: {
						nodeId: state.fromNode!.id,
						portId: state.fromHandle!.id ?? "",
					},
					to: {
						nodeId: state.toNode.id,
						portId: state.toHandle.id ?? "",
					},
					clientX,
					clientY,
				});
			} else if (!state.toNode && !state.isValid) {
				onconnectdrop?.({
					nodeId: state.fromNode!.id,
					portId: state.fromHandle!.id ?? "",
					clientX,
					clientY,
				});
			}
		}}
		onselectionchange={({ nodes: selected }) =>
			onnodeselect?.(selected.length === 1 ? selected[0].id : null)}
		onpaneclick={() => onpaneclick?.()}
		onnodecontextmenu={({ node, event }) => {
			event.preventDefault();
			onnodecontextmenu?.(node.id, event);
		}}
		onpanecontextmenu={({ event }) => {
			event.preventDefault();
			onpanecontextmenu?.(event);
		}}
		onedgecontextmenu={({ edge, event }) => {
			event.preventDefault();
			onedgecontextmenu?.(edge.id, event);
		}}
		onnodedragstop={() => onchange?.()}
		ondelete={({ nodes: gone, edges: lost }) => {
			if (gone.length) ondeletenodes?.(gone.map((n) => n.id));
			if (lost.length) ondeleteedges?.(lost.map((e) => e.id));
		}}
	>
		<Background
			variant={BackgroundVariant.Dots}
			gap={snapSize}
			size={1.4}
			patternColor="var(--border-strong)"
		/>
		<CanvasControls
			minZoom={BUILDER_ZOOM_LIMITS.minZoom}
			maxZoom={BUILDER_ZOOM_LIMITS.maxZoom}
			fitOptions={BUILDER_FIT_VIEW_OPTIONS}
		/>
		<MiniMap
			position="bottom-right"
			pannable
			zoomable
			nodeColor={minimapFill}
			class="!m-4 hidden !rounded-lg !border !border-border !bg-card !shadow-none md:block"
		/>
		{#if onready}<BuilderApiBridge {onready} />{/if}
	</SvelteFlow>
</div>

<style>
	.deepref-builder :global(.svelte-flow) {
		background-color: transparent;
		--xy-edge-stroke-default: var(--muted-foreground);
		--xy-edge-stroke-selected-default: var(--primary);
		--xy-edge-stroke-width-default: 2;
		--xy-minimap-background-color-default: var(--card);
		--xy-minimap-mask-background-color-default: color-mix(
			in oklab,
			var(--background) 60%,
			transparent
		);
	}
	.deepref-builder
		:global(.svelte-flow__edge.selected .svelte-flow__edge-path) {
		stroke: var(--primary);
	}
	.deepref-builder :global(.svelte-flow__connection-path) {
		stroke: var(--primary);
		stroke-width: 2;
	}
	.deepref-builder :global(.svelte-flow__handle) {
		transition: transform 0.12s ease;
	}
	.deepref-builder :global(.svelte-flow__handle:hover),
	.deepref-builder :global(.svelte-flow__handle.connectingto) {
		transform: translate(0, -50%) scale(1.35);
	}
	.deepref-builder :global(.svelte-flow__node) {
		cursor: grab;
	}
	.deepref-builder :global(.svelte-flow__pane.draggable) {
		cursor: default;
	}
	/* Attribution stays visible but quiet. Hiding it is a Svelte Flow Pro feature. */
	.deepref-builder :global(.svelte-flow__attribution) {
		background: transparent;
		padding: 0 0.25rem;
		margin: 0;
		font-size: 0.625rem;
		line-height: 1rem;
		color: var(--muted-foreground);
		opacity: 0.8;
	}
	.deepref-builder :global(.svelte-flow__attribution a) {
		color: inherit;
		text-decoration: none;
	}
	.deepref-builder :global(.svelte-flow__attribution a:hover) {
		text-decoration: underline;
	}
</style>
