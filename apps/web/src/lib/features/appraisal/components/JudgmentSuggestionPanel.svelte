<script lang="ts">
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import { differsFromSuggestion } from '../suggestion';

	type Props = {
		/** Prefix for element ids and test ids: a domain id or "overall". */
		idPrefix: string;
		status: 'pending' | 'ready' | 'unavailable';
		/** Suggested judgment value, or null when the answers do not yet decide one. */
		suggested: string | null | undefined;
		suggestedLabel: string | undefined;
		/** Wording of the questions that set the suggestion. */
		drivers: string[];
		/** Checks the reviewer should make before accepting the suggestion. */
		notes?: string[];
		chosen: string | undefined;
		chosenLabel: string | undefined;
		reason: string;
		onApply: () => void;
		onReasonChange: (value: string) => void;
	};

	let {
		idPrefix,
		status,
		suggested,
		suggestedLabel,
		drivers,
		notes = [],
		chosen,
		chosenLabel,
		reason,
		onApply,
		onReasonChange
	}: Props = $props();

	const differs = $derived(differsFromSuggestion(suggested, chosen));
</script>

<div class="flex min-w-0 flex-col gap-2 text-xs" data-testid={`${idPrefix}-suggestion`}>
	{#if status === 'pending'}
		<p class="text-muted-foreground">Checking the rule suggestion…</p>
	{:else if status === 'unavailable'}
		<p class="text-muted-foreground">
			The rule suggestion is unavailable right now. Your judgment is still saved as chosen.
		</p>
	{:else if !suggested}
		<p class="text-muted-foreground">
			Answer every question in this section to see a rule suggestion.
		</p>
	{:else}
		<div
			class="flex min-w-0 flex-col gap-2 rounded-lg border border-primary/15 bg-primary/5 p-3"
		>
			<p class="min-w-0 break-words">
				<span class="font-medium">Rule suggestion</span>
				<Badge size="sm" variant="outline">{suggestedLabel ?? suggested}</Badge>
				{#if drivers.length > 0}
					<span class="text-muted-foreground">set by: {drivers.join('; ')}</span>
				{/if}
			</p>
			{#if notes.length > 0}
				<ul
					class="flex min-w-0 list-disc flex-col gap-1 pl-4 text-muted-foreground"
					data-testid={`${idPrefix}-notes`}
				>
					{#each notes as note (note)}<li class="break-words">{note}</li>{/each}
				</ul>
			{/if}
			{#if !chosen}
				<div>
					<Button type="button" variant="outline" size="sm" onclick={onApply}
						>Use suggestion</Button
					>
				</div>
			{:else if differs}
				<p class="text-muted-foreground" data-testid={`${idPrefix}-differs`}>
					You chose {chosenLabel ?? chosen}, which differs from the suggestion. Explain
					why to keep your choice, or use the suggestion instead.
				</p>
				<div class="flex flex-wrap gap-2">
					<Button type="button" variant="outline" size="sm" onclick={onApply}
						>Use suggestion</Button
					>
				</div>
				<label for={`${idPrefix}-override-reason`} class="font-medium"
					>Reason for overriding the suggestion<span aria-hidden="true"> *</span></label
				>
				<textarea
					id={`${idPrefix}-override-reason`}
					rows="3"
					maxlength="1000"
					class="w-full rounded-lg border border-border/80 bg-background p-3 text-sm shadow-xs transition outline-none focus-visible:border-ring focus-visible:ring-3 focus-visible:ring-ring/30"
					value={reason}
					oninput={(event) => onReasonChange(event.currentTarget.value)}></textarea>
			{:else}
				<p class="text-muted-foreground">Matches your choice.</p>
			{/if}
		</div>
	{/if}
</div>
