<script lang="ts">
	import type { NodeProps } from "@xyflow/svelte";
	import type { BuilderNodeData } from "./types.js";

	let { data, selected }: NodeProps & { data: BuilderNodeData } = $props();
</script>

<div
	class={[
		"w-56 rounded-md border bg-warning-surface p-3 text-xs leading-relaxed text-foreground shadow-sm select-none",
		selected ? "border-primary ring-2 ring-primary/30" : "border-warning-border",
	]}
	data-testid="builder-note"
>
	<p class="whitespace-pre-wrap">{data.title}</p>
</div>
