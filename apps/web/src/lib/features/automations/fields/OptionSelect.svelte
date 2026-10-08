<script lang="ts">
	import * as Select from '@deepref/ui/select';

	let {
		options,
		value,
		placeholder = 'Choose…',
		label,
		onchange,
		class: className = 'w-full'
	}: {
		options: { value: string; label: string }[];
		value: string;
		placeholder?: string;
		label?: string;
		onchange: (value: string) => void;
		class?: string;
	} = $props();

	const current = $derived(
		options.find((option) => option.value === value)?.label ?? placeholder
	);
</script>

<Select.Root type="single" {value} onValueChange={(next) => next && onchange(next)}>
	<Select.Trigger class={className} aria-label={label}>{current}</Select.Trigger>
	<Select.Content>
		<Select.Group>
			{#each options as option (option.value)}
				<Select.Item value={option.value} label={option.label} />
			{/each}
		</Select.Group>
	</Select.Content>
</Select.Root>
