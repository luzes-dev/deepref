<script lang="ts">
	import type {
		FullTextExclusionReasonDto,
		ScreeningDecisionInput
	} from '$lib/api/generated/models';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';

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

<div class="flex flex-col gap-4" aria-label="Full-text decision controls">
	<div class="flex flex-wrap items-center gap-2">
		<Button disabled={!available || pending} onclick={() => onDecision('include', null)}
			>Include</Button
		>
		<Button
			variant="destructive"
			disabled={!canExclude || pending}
			onclick={() => onDecision('exclude', selectedReason)}
		>
			Exclude
		</Button>
		<Button
			variant="outline"
			disabled={!available || pending}
			onclick={() => onDecision('maybe', null)}>Maybe</Button
		>
		<Button variant="ghost" disabled={!canUndo || pending} onclick={onUndo}>Undo</Button>
		{#if !available}<Badge variant="secondary"
				>Decisions unlock when parsed full text is available</Badge
			>{/if}
	</div>
	<label class="flex max-w-md flex-col gap-2 text-sm" for="full-text-reason">
		<span class="font-medium">Primary full-text exclusion reason</span>
		<select
			id="full-text-reason"
			class="h-9 rounded-md border bg-background px-3"
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
			Exclude requires exactly one project full-text reason. Include and Maybe send no reason.
		</span>
	</label>
</div>
