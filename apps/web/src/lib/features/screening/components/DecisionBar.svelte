<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import type { ScreeningDecisionInput } from '$lib/api/generated/models';

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

<div class="flex flex-wrap items-center gap-2 border-t pt-4" aria-label="Screening decision">
	<Button
		variant="default"
		class="min-w-28"
		disabled={disabled || pending}
		onclick={() => onDecision('include')}
	>
		Include <kbd class="ml-1 rounded bg-primary-foreground/15 px-1.5 py-0.5 text-xs">I</kbd>
	</Button>
	<Button
		variant="destructive"
		class="min-w-28"
		disabled={disabled || pending}
		onclick={() => onDecision('exclude')}
	>
		Exclude <kbd class="bg-destructive-foreground/15 ml-1 rounded px-1.5 py-0.5 text-xs">E</kbd>
	</Button>
	<Button
		variant="outline"
		class="min-w-28"
		disabled={disabled || pending}
		onclick={() => onDecision('maybe')}
	>
		Maybe <kbd class="ml-1 rounded bg-muted px-1.5 py-0.5 text-xs">M</kbd>
	</Button>
	<Button
		variant="ghost"
		class="min-w-24"
		disabled={disabled || pending || !canUndo}
		aria-label="Undo latest screening decision"
		onclick={() => onUndo?.()}
	>
		Undo <kbd class="ml-1 rounded bg-muted px-1.5 py-0.5 text-xs">U</kbd>
	</Button>
	{#if pending}
		<span class="text-sm text-muted-foreground" aria-live="polite">Saving decision…</span>
	{:else}
		<span class="ml-auto text-xs text-muted-foreground">I / E / M decide · U undo</span>
	{/if}
</div>
