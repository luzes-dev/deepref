<script lang="ts">
	import { Handle, Position, type NodeProps } from '@xyflow/svelte';
	import CogIcon from '@lucide/svelte/icons/cog';
	import CheckCircle2Icon from '@lucide/svelte/icons/check-circle-2';
	import Loader2Icon from '@lucide/svelte/icons/loader-2';
	import AlertCircleIcon from '@lucide/svelte/icons/alert-circle';
	import PlayIcon from '@lucide/svelte/icons/play';
	import CopyIcon from '@lucide/svelte/icons/copy';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import type { ActionNodeData } from '../types.js';

	let { data, selected }: NodeProps & { data: ActionNodeData } = $props();

	const isCompleted = $derived(data.status === 'completed');
	const isRunning = $derived(data.status === 'running');
	const isFailed = $derived(data.status === 'failed');

	let speedVal = $state('1.0x');
</script>

<div
	class={[
		'group relative max-w-[290px] min-w-[250px] rounded-lg border bg-card text-card-foreground shadow-none transition-all duration-200 select-none',
		selected
			? 'border-primary ring-2  ring-primary/30'
			: 'border-border hover:border-primary/60 ',
		isRunning ? 'border-warning/80 ' : ''
	]}
>
	<!-- Input Handle (Left) -->
	<Handle
		type="target"
		position={Position.Left}
		class="!size-3 !border-2 !border-border !bg-primary transition-transform hover:!scale-125"
	/>

	<!-- Top Header: Icon + Title + Mini Action Buttons -->
	<div
		class="flex items-center justify-between gap-2 rounded-t-lg border-b border-border bg-card px-3 py-2"
	>
		<div class="flex min-w-0 items-center gap-2">
			<div
				class="flex size-5 shrink-0 items-center justify-center rounded bg-info/20 text-info"
			>
				{#if isRunning}
					<Loader2Icon class="size-3 animate-spin" />
				{:else if isCompleted}
					<CheckCircle2Icon class="size-3 text-success" />
				{:else if isFailed}
					<AlertCircleIcon class="size-3 text-destructive" />
				{:else}
					<CogIcon class="size-3" />
				{/if}
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
				class="flex size-6 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-primary/20 hover:text-foreground"
				title="Run step"
			>
				<PlayIcon class="size-2.5 fill-current" />
			</button>
			<button
				type="button"
				class="flex size-6 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-primary/20 hover:text-foreground"
				title="Duplicate step"
			>
				<CopyIcon class="size-2.5" />
			</button>
			<button
				type="button"
				class="flex size-6 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-destructive-surface hover:text-destructive"
				title="Delete step"
			>
				<Trash2Icon class="size-2.5" />
			</button>
		</div>
	</div>

	<!-- Content Body: Tactile Controls & Matrix Status -->
	<div class="space-y-2.5 p-3">
		{#if data.description}
			<p class="line-clamp-2 text-[11px] leading-relaxed text-muted-foreground">
				{data.description}
			</p>
		{/if}

		<!-- Interactive Slider Control -->
		<div class="space-y-1 rounded-lg border border-border bg-card p-2">
			<div class="flex items-center justify-between text-[10px]">
				<span class="font-mono text-muted-foreground">Execution Rate</span>
				<span class="font-mono font-semibold text-primary">{speedVal}</span>
			</div>
			<div class="flex items-center gap-2">
				<span class="font-mono text-[9px] text-muted-foreground">0.1x</span>
				<input
					type="range" aria-label="Execution rate"
					min="1"
					max="5"
					value="3"
					oninput={(e) =>
						(speedVal = `${(parseFloat((e.target as HTMLInputElement).value) * 0.4).toFixed(1)}x`)}
					class="h-1 w-full cursor-pointer appearance-none rounded-lg bg-card accent-primary"
				/>
				<span class="font-mono text-[9px] text-muted-foreground">2.0x</span>
			</div>
		</div>

		<!-- Status & Telemetry Footer -->
		<div class="flex items-center justify-between border-t border-border pt-1 text-[10px]">
			<span class="truncate font-mono text-muted-foreground">
				{data.actionKey}
			</span>
			<div class="flex shrink-0 items-center gap-1.5">
				{#if data.duration}
					<span
						class="rounded bg-card px-1.5 py-0.5 font-mono text-muted-foreground"
					>
						{data.duration}
					</span>
				{/if}
				<span
					class={[
						'rounded-full px-2 py-0.5 font-medium',
						isCompleted
							? 'bg-success/20 text-success'
							: isRunning
								? 'bg-warning/20 text-warning'
								: isFailed
									? 'bg-destructive-surface text-destructive'
									: 'bg-card text-muted-foreground'
					]}
				>
					{data.status ?? 'idle'}
				</span>
			</div>
		</div>
	</div>

	<!-- Output Handle (Right) -->
	<Handle
		type="source"
		position={Position.Right}
		class="!size-3 !border-2 !border-border !bg-primary transition-transform hover:!scale-125"
	/>
</div>
