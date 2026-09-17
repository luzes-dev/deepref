<script lang="ts">
	import { Handle, Position, type NodeProps } from '@xyflow/svelte';
	import ShieldCheckIcon from '@lucide/svelte/icons/shield-check';
	import UserCheckIcon from '@lucide/svelte/icons/user-check';
	import PlayIcon from '@lucide/svelte/icons/play';
	import CopyIcon from '@lucide/svelte/icons/copy';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import type { HumanGateNodeData } from '../types.js';

	let { data, selected }: NodeProps & { data: HumanGateNodeData } = $props();

	let confidenceThreshold = $state('85%');
</script>

<div
	class={[
		'group relative max-w-[300px] min-w-[260px] rounded-lg border bg-card text-card-foreground shadow-none transition-all duration-200 select-none',
		selected
			? 'border-destructive ring-2  ring-destructive/30'
			: 'border-border hover:border-destructive/60 '
	]}
>
	<!-- Input Handle (Left) -->
	<Handle
		type="target"
		position={Position.Left}
		class="!size-3 !border-2 !border-border !bg-destructive transition-transform hover:!scale-125"
	/>

	<!-- Top Header: Icon + Title + Mini Action Buttons -->
	<div
		class="flex items-center justify-between gap-2 rounded-t-lg border-b border-border bg-card px-3 py-2"
	>
		<div class="flex min-w-0 items-center gap-2">
			<div
				class="flex size-5 shrink-0 items-center justify-center rounded bg-destructive-surface text-destructive"
			>
				<ShieldCheckIcon class="size-3" />
			</div>
			<span class="truncate text-xs font-semibold tracking-tight text-foreground">
				{data.label}
			</span>
		</div>

		<!-- Mini action buttons: Play, Copy, Trash -->
		<div
			class="flex shrink-0 items-center gap-1 opacity-80 transition-opacity group-hover:opacity-100"
		>
			<button
				type="button"
				class="flex size-6 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-destructive-surface hover:text-destructive"
				title="Simulate approval"
			>
				<PlayIcon class="size-2.5 fill-current" />
			</button>
			<button
				type="button"
				class="flex size-6 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-primary/20 hover:text-foreground"
				title="Duplicate gate"
			>
				<CopyIcon class="size-2.5" />
			</button>
			<button
				type="button"
				class="flex size-6 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-destructive-surface hover:text-destructive"
				title="Remove gate"
			>
				<Trash2Icon class="size-2.5" />
			</button>
		</div>
	</div>

	<!-- Content Body -->
	<div class="space-y-2.5 p-3">
		{#if data.description}
			<p class="line-clamp-2 text-[11px] leading-relaxed text-muted-foreground">
				{data.description}
			</p>
		{/if}

		<!-- Confidence Threshold Slider -->
		<div class="space-y-1 rounded-lg border border-border bg-card p-2">
			<div class="flex items-center justify-between text-[10px]">
				<span class="font-mono text-muted-foreground">Consensus Threshold</span>
				<span class="font-mono font-semibold text-destructive">{confidenceThreshold}</span>
			</div>
			<div class="flex items-center gap-2">
				<span class="font-mono text-[9px] text-muted-foreground">50%</span>
				<input
					type="range" aria-label="Confidence threshold"
					min="50"
					max="100"
					value="85"
					oninput={(e) =>
						(confidenceThreshold = `${(e.target as HTMLInputElement).value}%`)}
					class="h-1 w-full cursor-pointer appearance-none rounded-lg bg-card accent-destructive"
				/>
				<span class="font-mono text-[9px] text-muted-foreground">100%</span>
			</div>
		</div>

		<!-- Footer -->
		<div class="flex items-center justify-between border-t border-border pt-1 text-[10px]">
			<div class="flex items-center gap-1 text-muted-foreground">
				<UserCheckIcon class="size-3 text-destructive" />
				<span class="truncate font-mono">{data.assignee ?? 'Assigned Reviewer'}</span>
			</div>
			<span class="rounded-full bg-destructive-surface px-2 py-0.5 font-medium text-destructive">
				{data.status ?? 'pending'}
			</span>
		</div>
	</div>

	<!-- Output Handle (Right) -->
	<Handle
		type="source"
		position={Position.Right}
		class="!size-3 !border-2 !border-border !bg-destructive transition-transform hover:!scale-125"
	/>
</div>
