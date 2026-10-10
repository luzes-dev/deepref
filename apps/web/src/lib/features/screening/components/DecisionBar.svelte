<script lang="ts">
	import { Button } from '@deepref/ui/button';
	import type { ScreeningDecisionInput } from '#lib/api/generated/models/index.js';
	import { Check, CircleHelp, RotateCcw, X } from '@lucide/svelte';
	import DecisionChoice from './DecisionChoice.svelte';

	let {
		disabled = false,
		pending = false,
		current,
		onDecision,
		onUndo,
		canUndo = false
	}: {
		disabled?: boolean;
		pending?: boolean;
		/** The report's recorded decision; the matching button shows as selected. */
		current?: string;
		onDecision: (decision: ScreeningDecisionInput) => void | Promise<void>;
		onUndo?: () => void | Promise<void>;
		canUndo?: boolean;
	} = $props();

	const options = [
		{ value: 'include', label: 'Include', key: 'I', icon: Check },
		{ value: 'exclude', label: 'Exclude', key: 'E', icon: X },
		{ value: 'maybe', label: 'Maybe', key: 'M', icon: CircleHelp }
	] as const;
</script>

<section
	class="flex items-center gap-2"
	aria-label="Screening decision"
	data-testid="screening-decision"
>
	{#each options as option (option.value)}
		<DecisionChoice
			decision={option.value}
			label={option.label}
			shortcut={option.key}
			icon={option.icon}
			selected={current === option.value}
			disabled={disabled || pending}
			onclick={() => onDecision(option.value)}
		/>
	{/each}
	<div class="ml-auto flex items-center gap-1 sm:gap-3">
		{#if pending}
			<span class="text-xs font-medium text-primary" aria-live="polite">Saving…</span>
		{/if}
		<Button
			variant="ghost"
			size="sm"
			disabled={disabled || pending || !canUndo}
			aria-label="Undo latest screening decision"
			onclick={() => onUndo?.()}
		>
			<RotateCcw data-icon="inline-start" /><span class="max-sm:sr-only">Undo</span><kbd
				aria-hidden="true">U</kbd
			>
		</Button>
	</div>
</section>

<style>
	kbd {
		display: none;
		min-width: 1.25rem;
		justify-content: center;
		border-radius: 0.25rem;
		background: color-mix(in oklab, currentColor 12%, transparent);
		padding: 0 0.3rem;
		font-family: var(--font-sans);
		font-size: 0.6875rem;
		font-weight: 600;
	}
	@media (min-width: 40rem) {
		kbd {
			display: inline-flex;
		}
	}
</style>
