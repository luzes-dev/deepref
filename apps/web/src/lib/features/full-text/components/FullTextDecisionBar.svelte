<script lang="ts">
	import type {
		FullTextExclusionReasonDto,
		ScreeningDecisionInput
	} from '$lib/api/generated/models';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Check, CircleHelp, RotateCcw, X } from '@lucide/svelte';

	let {
		reasons,
		selectedReason,
		pending = false,
		canUndo = false,
		available = false,
		onReasonChange,
		onDecision,
		onUndo
	}: {
		reasons: FullTextExclusionReasonDto[];
		selectedReason: string;
		pending?: boolean;
		canUndo?: boolean;
		available?: boolean;
		onReasonChange: (reasonId: string) => void;
		onDecision: (decision: ScreeningDecisionInput, reasonId: string | null) => void;
		onUndo: () => void;
	} = $props();

	const canExclude = $derived(available && selectedReason.length > 0);
</script>

<section
	class="flex flex-col gap-4 rounded-xl border border-primary/20 bg-primary/5 p-4"
	aria-label="Full-text decision controls"
>
	<div class="flex flex-wrap items-center justify-between gap-2">
		<div>
			<p class="text-sm font-semibold tracking-tight">Record full-text decision</p>
			<p class="text-xs text-muted-foreground">
				Choose one outcome after reviewing the parsed evidence.
			</p>
		</div>
		{#if pending}<span class="text-xs font-medium text-primary" aria-live="polite"
				>Saving decision…</span
			>{/if}
	</div>
	<div class="grid gap-2 sm:grid-cols-3">
		<Button
			class="min-h-11 justify-between bg-success text-background hover:bg-success/90"
			disabled={!available || pending}
			onclick={() => onDecision('include', null)}
		>
			<Check data-icon="inline-start" /> <span>Include</span> <kbd aria-hidden="true">I</kbd>
		</Button>
		<Button
			variant="destructive"
			class="min-h-11 justify-between"
			disabled={!canExclude || pending}
			onclick={() => onDecision('exclude', selectedReason)}
		>
			<X data-icon="inline-start" /> <span>Exclude</span> <kbd aria-hidden="true">E</kbd>
		</Button>
		<Button
			variant="outline"
			class="min-h-11 justify-between border-warning/50 bg-warning/10 text-foreground hover:bg-warning/20"
			disabled={!available || pending}
			onclick={() => onDecision('maybe', null)}
			><CircleHelp data-icon="inline-start" /> <span>Maybe</span>
			<kbd aria-hidden="true">M</kbd></Button
		>
	</div>
	<div class="flex flex-col gap-2 border-t border-primary/15 pt-3">
		<label class="flex max-w-md flex-col gap-2 text-sm" for="full-text-reason">
			<span class="font-medium">Primary full-text exclusion reason</span>
			<select
				id="full-text-reason"
				class="h-9 rounded-md border bg-background px-3 outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/50"
				value={selectedReason}
				onchange={(event) => onReasonChange(event.currentTarget.value)}
				aria-describedby="full-text-reason-help"
			>
				<option value="">Choose only for Exclude…</option>
				{#each reasons as reason (reason.id)}
					<option value={reason.id}>{reason.label}</option>
				{/each}
			</select>
			<span id="full-text-reason-help" class="text-xs text-muted-foreground">
				Exclude requires exactly one project full-text reason. Include and Maybe send no
				reason.
			</span>
		</label>
		<div class="flex flex-wrap items-center justify-between gap-2">
			<Button
				variant="ghost"
				class="min-h-10 px-0 text-muted-foreground hover:bg-transparent hover:text-foreground"
				disabled={!canUndo || pending}
				onclick={onUndo}
			>
				<RotateCcw data-icon="inline-start" /> Undo latest decision
				<kbd aria-hidden="true">U</kbd>
			</Button>
			{#if !available}<Badge variant="secondary"
					>Decisions unlock when parsed full text is available</Badge
				>{/if}
		</div>
	</div>
</section>

<style>
	kbd {
		display: inline-flex;
		min-width: 1.5rem;
		align-items: center;
		justify-content: center;
		border: 1px solid color-mix(in oklab, var(--border) 80%, transparent);
		border-radius: 0.3rem;
		background: var(--muted);
		padding: 0.1rem 0.35rem;
		font-size: 0.6875rem;
		font-weight: 600;
	}
</style>
