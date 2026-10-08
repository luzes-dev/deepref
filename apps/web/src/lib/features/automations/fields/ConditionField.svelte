<script lang="ts">
	import PlusIcon from '@lucide/svelte/icons/plus';
	import XIcon from '@lucide/svelte/icons/x';
	import { Button } from '@deepref/ui/button';
	import { Input } from '@deepref/ui/input';
	import type { FieldDetailsDto } from '$lib/api/generated/models';
	import { OPERATORS, type ConditionValue } from '../model';
	import OptionSelect from './OptionSelect.svelte';

	let {
		value,
		onchange,
		details
	}: {
		value: unknown;
		onchange: (value: ConditionValue) => void;
		/** What the rules can look at: the names the step before provides. */
		details?: FieldDetailsDto;
	} = $props();

	const listId = `look-at-${Math.random().toString(36).slice(2, 8)}`;

	const condition = $derived<ConditionValue>(
		typeof value === 'object' &&
			value !== null &&
			Array.isArray((value as ConditionValue).rules)
			? (value as ConditionValue)
			: { match: 'all', rules: [] }
	);
	const suggestions = $derived(details?.fields ?? []);

	function patchRule(index: number, change: Record<string, unknown>) {
		onchange({
			...condition,
			rules: condition.rules.map((rule, position) =>
				position === index ? { ...rule, ...change } : rule
			)
		});
	}
	function addRule() {
		onchange({
			...condition,
			rules: [...condition.rules, { field: 'title', operator: 'contains', value: '' }]
		});
	}
	function removeRule(index: number) {
		onchange({
			...condition,
			rules: condition.rules.filter((_, position) => position !== index)
		});
	}
</script>

<div class="flex flex-col gap-2" data-testid="condition-field">
	{#if condition.rules.length > 1}
		<div class="flex items-center gap-2 text-sm">
			Keep when
			<OptionSelect
				class="w-28"
				label="Match"
				value={condition.match}
				options={[
					{ value: 'all', label: 'all' },
					{ value: 'any', label: 'any' }
				]}
				onchange={(match) => onchange({ ...condition, match: match as 'all' | 'any' })}
			/>
			of these are true
		</div>
	{/if}
	{#each condition.rules as rule, index (index)}
		{@const needsValue =
			OPERATORS.find((item) => item.value === rule.operator)?.needsValue ?? true}
		<div class="flex flex-col gap-1.5 rounded-md border border-border bg-muted/40 p-2">
			<div class="flex items-center gap-1.5">
				<Input
					aria-label="Look at"
					placeholder="Name, for example title"
					list={listId}
					value={rule.field}
					oninput={(event) => patchRule(index, { field: event.currentTarget.value })}
				/>
				<Button
					type="button"
					size="icon-xs"
					variant="ghost"
					aria-label="Remove this rule"
					onclick={() => removeRule(index)}><XIcon /></Button
				>
			</div>
			<OptionSelect
				label="Condition"
				value={rule.operator}
				options={OPERATORS}
				onchange={(operator) => patchRule(index, { operator })}
			/>
			{#if needsValue}
				<Input
					aria-label="Value"
					placeholder="Value"
					value={String(rule.value ?? '')}
					oninput={(event) => {
						const raw = event.currentTarget.value;
						const numeric =
							raw.trim() !== '' && Number.isFinite(Number(raw)) ? Number(raw) : raw;
						patchRule(index, { value: numeric });
					}}
				/>
			{/if}
		</div>
	{/each}
	{#if suggestions.length > 0}
		<datalist id={listId}>
			{#each suggestions as item (item.key)}
				<option value={item.key} label={item.label}></option>
			{/each}
		</datalist>
	{/if}
	{#if details?.list}
		<p class="text-xs text-muted-foreground">
			The step before gives a list, so rules can test its size with “count”.
		</p>
	{:else if details?.open}
		<p class="text-xs text-muted-foreground">
			Its names depend on what is sent. Type the name to test, for example status.
		</p>
	{/if}
	<Button type="button" size="sm" variant="outline" onclick={addRule}>
		<PlusIcon /> Add a rule
	</Button>
</div>
