<script lang="ts">
	import { getContext } from "svelte";
	import { Handle, Position, type NodeProps } from "@xyflow/svelte";
	import PlusIcon from "@lucide/svelte/icons/plus";
	import AlertTriangleIcon from "@lucide/svelte/icons/triangle-alert";
	import CheckIcon from "@lucide/svelte/icons/check";
	import XIcon from "@lucide/svelte/icons/x";
	import LoaderIcon from "@lucide/svelte/icons/loader-circle";
	import { BUILDER_CONTEXT, type BuilderContext } from "./builder-context.js";
	import type { BuilderNodeData, BuilderTone } from "./types.js";

	let { id, data, selected }: NodeProps & { data: BuilderNodeData } = $props();
	const builder = getContext<BuilderContext | undefined>(BUILDER_CONTEXT);

	const toneChip: Record<BuilderTone, string> = {
		trigger: "bg-success/15 text-success",
		data: "bg-info/15 text-info",
		action: "bg-primary/15 text-primary",
		ai: "bg-chart-5/15 text-chart-5",
		logic: "bg-chart-2/15 text-chart-2",
		integration: "bg-chart-4/15 text-chart-4",
		note: "bg-warning/15 text-warning",
	};
	const toneStripe: Record<BuilderTone, string> = {
		trigger: "border-l-success",
		data: "border-l-info",
		action: "border-l-primary",
		ai: "border-l-chart-5",
		logic: "border-l-chart-2",
		integration: "border-l-chart-4",
		note: "border-l-warning",
	};
	const statusLabel: Record<string, string> = {
		queued: "Waiting",
		running: "Running",
		completed: "Done",
		failed: "Problem",
		skipped: "Skipped",
		cancelled: "Cancelled",
	};

	const issues = $derived(data.issues ?? []);
	const Icon = $derived(data.iconKey ? builder?.iconFor(data.iconKey) : undefined);
	const connected = $derived(new Set(data.connectedOutputs ?? []));
	const ring = $derived(
		data.status === "failed"
			? "border-destructive"
			: data.status === "completed"
				? "border-success"
				: data.status === "running"
					? "border-primary"
					: selected
						? "border-primary ring-2 ring-primary/30"
						: issues.length > 0
							? "border-warning"
							: "border-border hover:border-border-strong",
	);
</script>

<div
	class={[
		"group relative w-64 rounded-lg border border-l-4 bg-card text-card-foreground shadow-sm transition-colors select-none",
		toneStripe[data.tone],
		ring,
		data.status === "skipped" && "opacity-60",
	]}
	data-testid="builder-node"
	data-node-title={data.title}
>
	<div class="flex items-start gap-2.5 px-3 pt-2.5 pb-2">
		<span
			class={[
				"flex size-7 shrink-0 items-center justify-center rounded-md",
				toneChip[data.tone],
			]}
		>
			{#if Icon}<Icon class="size-4" />{/if}
		</span>
		<div class="min-w-0 flex-1">
			{#if data.kind}
				<p class="truncate text-3xs tracking-snug-caps text-muted-foreground uppercase">
					{data.kind}
				</p>
			{/if}
			<p class="text-sm leading-snug font-semibold text-foreground">{data.title}</p>
			{#if data.summary}
				<p class="mt-0.5 line-clamp-2 text-xs leading-snug text-muted-foreground">
					{data.summary}
				</p>
			{/if}
		</div>
		{#if data.status}
			<span
				class={[
					"flex shrink-0 items-center gap-1 rounded-full px-1.5 py-0.5 text-3xs font-medium",
					data.status === "failed" && "bg-destructive/15 text-destructive",
					data.status === "completed" && "bg-success/15 text-success",
					data.status === "running" && "bg-primary/15 text-primary",
					(data.status === "skipped" ||
						data.status === "queued" ||
						data.status === "cancelled") &&
						"bg-muted text-muted-foreground",
				]}
			>
				{#if data.status === "completed"}<CheckIcon class="size-2.5" />
				{:else if data.status === "failed"}<XIcon class="size-2.5" />
				{:else if data.status === "running"}<LoaderIcon class="size-2.5 animate-spin" />{/if}
				{statusLabel[data.status]}
			</span>
		{/if}
	</div>

	{#if issues.length > 0}
		<div
			class="mx-3 mb-2 flex items-start gap-1.5 rounded-md bg-warning/10 px-2 py-1 text-2xs text-warning"
			title={issues.join("\n")}
			data-testid="builder-node-issue"
		>
			<AlertTriangleIcon class="mt-px size-3 shrink-0" />
			<span class="line-clamp-2">{issues[0]}</span>
			{#if issues.length > 1}<span class="shrink-0">+{issues.length - 1}</span>{/if}
		</div>
	{/if}

	{#if data.preview}
		<p
			class="mx-3 mb-2 rounded-md bg-muted px-2 py-1 text-2xs text-foreground tabular-nums"
			data-testid="builder-node-preview"
		>
			{data.preview}
		</p>
	{/if}
	{#if data.dryRun}
		<p class="mx-3 mb-2 text-3xs text-muted-foreground italic">
			Test runs only pretend to do this
		</p>
	{/if}

	{#if data.inputs.length > 0 || data.outputs.length > 0}
		<div class="grid grid-cols-2 border-t border-border-subtle text-2xs text-muted-foreground">
			<div class="flex flex-col">
				{#each data.inputs as port (port.id)}
					<div class="relative flex min-h-6 items-center py-1 pr-1 pl-3">
						<Handle
							type="target"
							position={Position.Left}
							id={port.id}
							class="!size-3 !border-2 !border-card !bg-muted-foreground pointer-coarse:!size-6"
						/>
						<span class="truncate" title={port.label}>{port.label}</span>
					</div>
				{/each}
			</div>
			<div class="flex flex-col items-end">
				{#each data.outputs as port (port.id)}
					<div class="relative flex min-h-6 w-full items-center justify-end py-1 pr-3 pl-1">
						<span class="truncate" title={port.label}>{port.label}</span>
						<Handle
							type="source"
							position={Position.Right}
							id={port.id}
							class="!size-3 !border-2 !border-card !bg-primary pointer-coarse:!size-6"
						/>
						{#if data.addable && !connected.has(port.id)}
							<button
								type="button"
								class="nodrag nopan absolute top-1/2 -right-8 flex size-5 -translate-y-1/2 items-center justify-center rounded-full border border-border bg-card text-muted-foreground opacity-0 shadow-xs transition-opacity group-hover:opacity-100 hover:bg-primary hover:text-primary-foreground focus-visible:opacity-100 focus-visible:outline-2 focus-visible:outline-ring pointer-coarse:size-7 pointer-coarse:opacity-100"
								aria-label={`Add a block after ${port.label}`}
								title={`Add a block after ${port.label}`}
								onclick={(event) => builder?.addFrom(id, port.id, event.currentTarget)}
							>
								<PlusIcon class="size-3 pointer-coarse:size-4" />
							</button>
						{/if}
					</div>
				{/each}
			</div>
		</div>
	{/if}
</div>
