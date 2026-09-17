<script lang="ts">
	import { Handle, Position, type NodeProps } from '@xyflow/svelte';
	import GitBranchIcon from '@lucide/svelte/icons/git-branch';
	import PlayIcon from '@lucide/svelte/icons/play';
	import CopyIcon from '@lucide/svelte/icons/copy';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import type { ConditionNodeData } from '../types.js';

	let { data, selected }: NodeProps & { data: ConditionNodeData } = $props();
</script>

<div
	class={[
		'group relative max-w-[280px] min-w-[240px] rounded-lg border bg-card text-card-foreground shadow-none transition-all duration-200 select-none',
		selected
			? 'border-warning ring-2  ring-warning/30'
			: 'border-border hover:border-warning/60 '
	]}
>
	<!-- Input Handle (Left) -->
	<Handle
		type="target"
		position={Position.Left}
		class="!size-3 !border-2 !border-border !bg-warning transition-transform hover:!scale-125"
	/>

	<!-- Top Header: Icon + Title + Mini Action Buttons -->
	<div
		class="flex items-center justify-between gap-2 rounded-t-lg border-b border-border bg-card px-3 py-2"
	>
		<div class="flex min-w-0 items-center gap-2">
			<div
				class="flex size-5 shrink-0 items-center justify-center rounded bg-warning/20 text-warning"
			>
				<GitBranchIcon class="size-3" />
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
				class="flex size-6 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-warning/20 hover:text-warning"
				title="Evaluate condition"
			>
				<PlayIcon class="size-2.5 fill-current" />
			</button>
			<button
				type="button"
				class="flex size-6 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-primary/20 hover:text-foreground"
				title="Duplicate condition"
			>
				<CopyIcon class="size-2.5" />
			</button>
			<button
				type="button"
				class="flex size-6 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-destructive-surface hover:text-destructive"
				title="Delete condition"
			>
				<Trash2Icon class="size-2.5" />
			</button>
		</div>
	</div>

	<!-- Content Body -->
	<div class="space-y-2.5 p-3">
		{#if data.expression}
			<div class="rounded-lg border border-border bg-card p-2">
				<span class="font-mono text-[11px] leading-tight break-all text-warning/90">
					{data.expression}
				</span>
			</div>
		{/if}

		<!-- Branch Outputs (True / False) -->
		<div class="grid grid-cols-2 gap-2 pt-1">
			<div
				class="relative flex items-center justify-between rounded border border-success/40 bg-card px-2 py-1"
			>
				<span class="font-mono text-[10px] font-semibold text-success">
					{data.trueLabel ?? 'MATCH'}
				</span>
				<span class="size-1.5 rounded-full bg-success"></span>
				<Handle
					id="true"
					type="source"
					position={Position.Right}
					class="!size-2.5 !border-2 !border-border !bg-success"
				/>
			</div>

			<div
				class="relative flex items-center justify-between rounded border border-destructive/40 bg-card px-2 py-1"
			>
				<span class="font-mono text-[10px] font-semibold text-destructive">
					{data.falseLabel ?? 'BYPASS'}
				</span>
				<span class="size-1.5 rounded-full bg-destructive"></span>
				<Handle
					id="false"
					type="source"
					position={Position.Right}
					class="!size-2.5 !border-2 !border-border !bg-destructive"
				/>
			</div>
		</div>
	</div>
</div>
