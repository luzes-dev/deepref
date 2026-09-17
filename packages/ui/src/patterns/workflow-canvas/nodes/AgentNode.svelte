<script lang="ts">
	import { Handle, Position, type NodeProps } from '@xyflow/svelte';
	import BotIcon from '@lucide/svelte/icons/bot';
	import ShieldAlertIcon from '@lucide/svelte/icons/shield-alert';
	import SearchIcon from '@lucide/svelte/icons/search';
	import FileTextIcon from '@lucide/svelte/icons/file-text';
	import PlayIcon from '@lucide/svelte/icons/play';
	import CopyIcon from '@lucide/svelte/icons/copy';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import type { AgentNodeData } from '../types.js';

	let { data, selected }: NodeProps & { data: AgentNodeData } = $props();

	const isProposal = $derived(data.kind === 'proposal');
	const isAgent = $derived(data.kind === 'agent');

	let tempVal = $state('0.2');
</script>

<div
	class={[
		'group relative max-w-[300px] min-w-[260px] rounded-lg border bg-card text-card-foreground shadow-none transition-all duration-200 select-none',
		selected
			? 'border-primary ring-2  ring-primary/30'
			: 'border-border hover:border-primary/60 '
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
				class={[
					'flex size-5 shrink-0 items-center justify-center rounded',
					isAgent
						? 'bg-primary/20 text-primary'
						: isProposal
							? 'bg-destructive-surface text-destructive'
							: 'bg-cyan-500/20 text-cyan-400'
				]}
			>
				{#if isAgent}
					<BotIcon class="size-3" />
				{:else if isProposal}
					<ShieldAlertIcon class="size-3" />
				{:else if data.toolName?.includes('search')}
					<SearchIcon class="size-3" />
				{:else}
					<FileTextIcon class="size-3" />
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
				title="Inspect tool"
			>
				<PlayIcon class="size-2.5 fill-current" />
			</button>
			<button
				type="button"
				class="flex size-6 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-primary/20 hover:text-foreground"
				title="Duplicate node"
			>
				<CopyIcon class="size-2.5" />
			</button>
			<button
				type="button"
				class="flex size-6 items-center justify-center rounded text-muted-foreground transition-colors hover:bg-destructive-surface hover:text-destructive"
				title="Remove node"
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

		<!-- Interactive Parameter Slider / Matrix -->
		<div class="space-y-1.5 rounded-lg border border-border bg-card p-2">
			<div class="flex items-center justify-between text-[10px]">
				<span class="font-mono text-muted-foreground">
					{isProposal ? 'Authority Level' : 'Temperature'}
				</span>
				<span class="font-mono font-semibold text-primary">
					{isProposal ? 'scientific_conclusion' : tempVal}
				</span>
			</div>
			{#if !isProposal}
				<div class="flex items-center gap-2">
					<span class="font-mono text-[9px] text-muted-foreground">0.0</span>
					<input
						type="range" aria-label="Temperature"
						min="0"
						max="10"
						value="2"
						oninput={(e) =>
							(tempVal = (
								parseFloat((e.target as HTMLInputElement).value) / 10
							).toFixed(1))}
						class="h-1 w-full cursor-pointer appearance-none rounded-lg bg-card accent-primary"
					/>
					<span class="font-mono text-[9px] text-muted-foreground">1.0</span>
				</div>
			{:else}
				<div class="flex items-center gap-1.5 font-mono text-[9px] text-destructive/90">
					<span class="size-1.5 rounded-full bg-destructive"></span>
					Requires Human Review Gate
				</div>
			{/if}
		</div>

		<!-- Footer -->
		<div class="flex items-center justify-between border-t border-border pt-1 text-[10px]">
			<span class="truncate font-mono text-muted-foreground">
				{data.toolName}
			</span>
			<span
				class={[
					'rounded-full px-2 py-0.5 font-medium',
					isProposal
						? 'bg-destructive-surface text-destructive'
						: isAgent
							? 'bg-primary/20 text-primary'
							: 'bg-cyan-500/20 text-cyan-400'
				]}
			>
				{isProposal ? 'proposal' : isAgent ? 'copilot' : 'read_only'}
			</span>
		</div>
	</div>

	<!-- Output Handle (Right) -->
	<Handle
		type="source"
		position={Position.Right}
		class="!size-3 !border-2 !border-border !bg-primary transition-transform hover:!scale-125"
	/>
</div>
