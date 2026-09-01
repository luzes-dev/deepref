<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import type { ScreeningDecisionInput } from '$lib/api/generated/models';
	import { Check, CircleHelp, RotateCcw, X } from '@lucide/svelte';

	let {
		disabled = false,
		pending = false,
		onDecision,
		onUndo,
		canUndo = false
	}: {
		disabled?: boolean;
		pending?: boolean;
		onDecision: (decision: ScreeningDecisionInput) => void | Promise<void>;
		onUndo?: () => void | Promise<void>;
		canUndo?: boolean;
	} = $props();
</script>

<section
	class="flex flex-col gap-3 rounded-xl border border-primary/20 bg-primary/5 p-4"
	aria-label="Screening decision"
>
	<div class="flex flex-wrap items-center justify-between gap-2">
		<div>
			<p class="text-sm font-semibold tracking-tight">Make a screening decision</p>
			<p class="text-xs text-muted-foreground">
				Your choice is recorded against the active protocol.
			</p>
		</div>
		{#if pending}
			<span class="text-xs font-medium text-primary" aria-live="polite">Saving decision…</span
			>
		{:else}
			<span class="text-xs text-muted-foreground">I / E / M decide</span>
		{/if}
	</div>
	<div class="grid grid-cols-1 gap-2 sm:grid-cols-3">
		<Button
			variant="default"
			class="min-h-11 justify-between bg-success text-background hover:bg-success/90"
			disabled={disabled || pending}
			onclick={() => onDecision('include')}
		>
			<Check data-icon="inline-start" />
			<span>Include</span>
			<kbd aria-hidden="true" class="rounded bg-background/20 px-1.5 py-0.5 text-xs">I</kbd>
		</Button>
		<Button
			variant="destructive"
			class="min-h-11 justify-between"
			disabled={disabled || pending}
			onclick={() => onDecision('exclude')}
		>
			<X data-icon="inline-start" />
			<span>Exclude</span>
			<kbd
				aria-hidden="true"
				class="bg-destructive-foreground/15 rounded px-1.5 py-0.5 text-xs">E</kbd
			>
		</Button>
		<Button
			variant="outline"
			class="min-h-11 justify-between border-warning/50 bg-warning/10 text-foreground hover:bg-warning/20"
			disabled={disabled || pending}
			onclick={() => onDecision('maybe')}
		>
			<CircleHelp data-icon="inline-start" />
			<span>Maybe</span>
			<kbd aria-hidden="true" class="rounded bg-warning/20 px-1.5 py-0.5 text-xs">M</kbd>
		</Button>
	</div>
	<div class="flex flex-wrap items-center justify-between gap-2 border-t border-primary/15 pt-2">
		<Button
			variant="ghost"
			class="min-h-10 px-0 text-muted-foreground hover:bg-transparent hover:text-foreground"
			disabled={disabled || pending || !canUndo}
			aria-label="Undo latest screening decision"
			onclick={() => onUndo?.()}
		>
			<RotateCcw data-icon="inline-start" />
			Undo latest decision
			<kbd aria-hidden="true" class="ml-1 rounded bg-muted px-1.5 py-0.5 text-xs">U</kbd>
		</Button>
		<span class="text-[11px] text-muted-foreground">Reversible until the next review event</span
		>
	</div>
</section>
