<script lang="ts">
	import { Handle, Position, type NodeProps } from '@xyflow/svelte';
	import ZapIcon from '@lucide/svelte/icons/zap';
	import PlayIcon from '@lucide/svelte/icons/play';
	import ClockIcon from '@lucide/svelte/icons/clock';
	import CopyIcon from '@lucide/svelte/icons/copy';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import type { TriggerNodeData } from '../types.js';

	let { data, selected }: NodeProps & { data: TriggerNodeData } = $props();

	const isManual = $derived(data.triggerKey === 'manual');
	const isActive = $derived(data.status === 'active' || data.status === 'running');
</script>

<div
	class={[
		'group relative max-w-[290px] min-w-[250px] rounded-lg border bg-card text-card-foreground shadow-none transition-all duration-200 select-none',
		selected
			? 'border-success ring-2  ring-success/30'
			: 'border-border hover:border-success/60 '
	]}
>
	<!-- Top Header: Icon + Title + Mini Action Buttons -->
	<div
		class="flex items-center justify-between gap-2 rounded-t-lg border-b border-border bg-card px-3 py-2"
	>
		<div class="flex min-w-0 items-center gap-2">
			<div
				class="flex size-5 shrink-0 items-center justify-center rounded bg-success/20 text-success"
			>
				{#if isManual}
					<PlayIcon class="size-2.5 fill-current" />
				{:else if data.triggerKey?.includes('cron') || data.triggerKey?.includes('schedule')}
					<ClockIcon class="size-3" />
				{:else}
					<ZapIcon class="size-3 fill-current" />
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
				class="flex size-6 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-success/20 hover:text-success"
				title="Trigger test"
			>
				<PlayIcon class="size-2.5 fill-current" />
			</button>
			<button
				type="button"
				class="flex size-6 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-primary/20 hover:text-foreground"
				title="Duplicate trigger"
			>
				<CopyIcon class="size-2.5" />
			</button>
			<button
				type="button"
				class="flex size-6 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-destructive-surface hover:text-destructive"
				title="Delete trigger"
			>
				<Trash2Icon class="size-2.5" />
			</button>
		</div>
	</div>

	<!-- Content Body: Interactive Pads / Status Indicators -->
	<div class="space-y-2.5 p-3">
		{#if data.description}
			<p class="line-clamp-2 text-[11px] leading-relaxed text-muted-foreground">
				{data.description}
			</p>
		{/if}

		<!-- Interactive Trigger Pad Matrix -->
		<div class="space-y-1.5 rounded-lg border border-border bg-card p-2">
			<div class="flex items-center justify-between text-[10px]">
				<span class="font-mono text-muted-foreground">Listener Mode</span>
				<span class="font-mono font-semibold text-success">{data.triggerKey}</span>
			</div>
			<!-- Mini Sequencer Pad Indicators -->
			<div class="grid grid-cols-4 gap-1">
				{#each [0, 1, 2, 3] as padIdx (padIdx)}
					<div
						class={[
							'flex h-4 items-center justify-center rounded font-mono text-[9px] transition-colors',
							padIdx === 0
								? 'bg-success font-bold text-foreground shadow-sm '
								: 'bg-card text-muted-foreground'
						]}
					>
						{padIdx === 0 ? 'ON' : 'OFF'}
					</div>
				{/each}
			</div>
		</div>

		<!-- Footer -->
		<div class="flex items-center justify-between border-t border-border pt-1 text-[10px]">
			<span class="truncate font-mono text-muted-foreground">
				{isManual ? 'manual_dispatch' : 'event_webhook'}
			</span>
			<span
				class={[
					'rounded-full px-2 py-0.5 font-medium',
					isActive ? 'bg-success/20 text-success' : 'bg-card text-muted-foreground'
				]}
			>
				{data.status ?? 'active'}
			</span>
		</div>
	</div>

	<!-- Output Handle (Right) -->
	<Handle
		type="source"
		position={Position.Right}
		class="!size-3 !border-2 !border-border !bg-success transition-transform hover:!scale-125"
	/>
</div>
