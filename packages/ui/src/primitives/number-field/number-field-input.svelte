<script lang="ts">
	import { cn } from '../../internal/utils.js';
	import { useNumberFieldInput } from './number-field.svelte.js';
	import type { NumberFieldInputProps } from './types.js';

	let { ref = $bindable(null), class: className, ...rest }: NumberFieldInputProps = $props();

	const inputState = useNumberFieldInput();
</script>

<input
	class={cn(
		'h-control-height min-w-0 flex-1 rounded-md border border-input bg-card text-foreground px-4 text-center outline-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring aria-invalid:border-destructive',
		className
	)}
	bind:this={ref}
	data-slot="number-field-input"
	bind:value={inputState.rootState.opts.value.current}
	{...inputState.props}
	aria-label={rest['aria-label'] ?? 'Numeric value'}
	{...rest}
/>

<style>
	input[type='number'] {
		appearance: none;
		-moz-appearance: textfield;
	}

	input[type='number']::-webkit-outer-spin-button,
	input[type='number']::-webkit-inner-spin-button {
		-webkit-appearance: none;
		margin: 0;
	}
</style>
