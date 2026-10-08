<script lang="ts">
	import PlusIcon from '@lucide/svelte/icons/plus';
	import XIcon from '@lucide/svelte/icons/x';
	import { Button } from '@deepref/ui/button';
	import { Input } from '@deepref/ui/input';
	import OptionSelect from './OptionSelect.svelte';

	interface Row {
		name: string;
		kind: string;
		description?: string;
	}
	let { value, onchange }: { value: unknown; onchange: (value: Row[]) => void } = $props();

	const rows = $derived<Row[]>(Array.isArray(value) ? (value as Row[]) : []);
	const KINDS = [
		{ value: 'text', label: 'Text' },
		{ value: 'number', label: 'Number' },
		{ value: 'boolean', label: 'Yes / no' }
	];

	function patch(index: number, change: Partial<Row>) {
		onchange(rows.map((row, position) => (position === index ? { ...row, ...change } : row)));
	}
</script>

<div class="flex flex-col gap-2" data-testid="field-list-field">
	{#each rows as row, index (index)}
		<div class="flex flex-col gap-1.5 rounded-md border border-border bg-muted/40 p-2">
			<div class="flex items-center gap-1.5">
				<Input
					aria-label="Name of the answer"
					placeholder="Name of the answer"
					value={row.name}
					oninput={(event) => patch(index, { name: event.currentTarget.value })}
				/>
				<Button
					type="button"
					size="icon-xs"
					variant="ghost"
					aria-label="Remove this answer"
					onclick={() => onchange(rows.filter((_, position) => position !== index))}
					><XIcon /></Button
				>
			</div>
			<OptionSelect
				label="Kind of answer"
				value={row.kind || 'text'}
				options={KINDS}
				onchange={(kind) => patch(index, { kind })}
			/>
			<Input
				aria-label="What it means"
				placeholder="What should this answer contain? (optional)"
				value={row.description ?? ''}
				oninput={(event) => patch(index, { description: event.currentTarget.value })}
			/>
		</div>
	{/each}
	<Button
		type="button"
		size="sm"
		variant="outline"
		onclick={() => onchange([...rows, { name: '', kind: 'text', description: '' }])}
	>
		<PlusIcon /> Add an answer
	</Button>
</div>
