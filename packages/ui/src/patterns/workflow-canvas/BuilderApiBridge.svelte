<script lang="ts">
	import { useSvelteFlow } from "@xyflow/svelte";
	import { BUILDER_FIT_VIEW_OPTIONS } from "./viewport-options";
	import type { BuilderCanvasApi } from "./types.js";

	let { onready }: { onready: (api: BuilderCanvasApi) => void } = $props();

	const flow = useSvelteFlow();

	$effect(() => {
		onready({
			screenToFlow: (point) => flow.screenToFlowPosition(point),
			fitView: () => void flow.fitView({ ...BUILDER_FIT_VIEW_OPTIONS, duration: 250 }),
			getZoom: () => flow.getZoom(),
			revealNode: (id, area) => {
				const node = flow.getInternalNode(id);
				if (!node) return;
				const { x, y, zoom } = flow.getViewport();
				const pad = 24;
				const left = node.internals.positionAbsolute.x * zoom + x;
				const top = node.internals.positionAbsolute.y * zoom + y;
				const right = left + (node.measured.width ?? 200) * zoom;
				const bottom = top + (node.measured.height ?? 80) * zoom;
				let dx = 0;
				let dy = 0;
				if (right > area.width - pad) dx = area.width - pad - right;
				if (left + dx < pad) dx = pad - left;
				if (bottom > area.height - pad) dy = area.height - pad - bottom;
				if (top + dy < pad) dy = pad - top;
				if (dx !== 0 || dy !== 0) {
					void flow.setViewport({ x: x + dx, y: y + dy, zoom }, { duration: 200 });
				}
			},
		});
	});
</script>
