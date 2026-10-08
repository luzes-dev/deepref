<script lang="ts">
	import type {
		FullTextExclusionReasonDto,
		ScreeningDecisionInput
	} from '$lib/api/generated/models';
	import { Button } from '@deepref/ui/button';
	import { Check, CircleHelp, RotateCcw, X } from '@lucide/svelte';
	import DecisionChoice from '$lib/features/screening/components/DecisionChoice.svelte';

	let {
		reasons,
		current,
		pending = false,
		canUndo = false,
		available = false,
		statusMessage = '',
		choosingReason = $bindable(false),
		onDecision,
		onUndo
	}: {
		reasons: FullTextExclusionReasonDto[];
		/** The report's recorded full-text decision; the matching button shows as selected. */
		current?: string;
		pending?: boolean;
		canUndo?: boolean;
		available?: boolean;
		/** Subtle confirmation shown under the buttons (announced politely). */
		statusMessage?: string;
		/** Exclusion needs exactly one reason, so Exclude opens the reasons and a reason commits. */
		choosingReason?: boolean;
		onDecision: (decision: ScreeningDecisionInput, reasonId: string | null) => void;
		onUndo: () => void;
	} = $props();

	const options = [
		{ value: 'include', label: 'Include', key: 'I', icon: Check },
		{ value: 'exclude', label: 'Exclude', key: 'E', icon: X },
		{ value: 'maybe', label: 'Maybe', key: 'M', icon: CircleHelp }
	] as const;
</script>

<section
	class="flex flex-col gap-2"
	aria-label="Full-text decision controls"
	data-testid="full-text-decision"
>
	{#if choosingReason}
		<div class="flex flex-wrap items-center gap-2" role="group" aria-label="Exclusion reason">
			<span class="text-sm font-medium">Exclude because</span>
			{#each reasons as reason, index (reason.id)}
				<Button
					variant="outline"
					size="sm"
					disabled={pending}
					onclick={() => {
						choosingReason = false;
						onDecision('exclude', reason.id);
					}}
					>{reason.label}{#if index < 9}<kbd aria-hidden="true">{index + 1}</kbd
						>{/if}</Button
				>
			{:else}
				<span class="text-sm text-muted-foreground"
					>No full-text exclusion reasons are configured for this project.</span
				>
			{/each}
			<Button
				variant="ghost"
				size="sm"
				class="ml-auto"
				onclick={() => (choosingReason = false)}>Cancel</Button
			>
		</div>
	{:else}
		<div class="flex items-center gap-2">
			{#each options as option (option.value)}
				<DecisionChoice
					decision={option.value}
					label={option.label}
					shortcut={option.key}
					icon={option.icon}
					selected={current === option.value}
					disabled={!available || pending}
					onclick={() =>
						option.value === 'exclude'
							? (choosingReason = true)
							: onDecision(option.value, null)}
				/>
			{/each}
			<div class="ml-auto flex items-center gap-1 sm:gap-3">
				{#if pending}<span class="text-xs font-medium text-primary" aria-live="polite"
						>Saving…</span
					>{/if}
				<Button
					variant="ghost"
					size="sm"
					disabled={!canUndo || pending}
					aria-label="Undo latest full-text decision"
					onclick={onUndo}
				>
					<RotateCcw data-icon="inline-start" /><span class="max-sm:sr-only">Undo</span
					><kbd aria-hidden="true">U</kbd>
				</Button>
			</div>
		</div>
		{#if statusMessage}
			<p class="text-xs text-muted-foreground" role="status" aria-live="polite">
				{statusMessage}
			</p>
		{/if}
		{#if !available}
			<p class="text-xs text-muted-foreground">
				Decisions unlock once a PDF is attached and parsed.
			</p>
		{/if}
	{/if}
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
