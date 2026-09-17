<script lang="ts">
	import { useStepperRoot } from '../../primitives/stepper/stepper.svelte.js';
	import type { StepperRootProps } from '../../primitives/stepper/types';
	import { box } from 'svelte-toolbelt';

	let { step = $bindable(1), children }: StepperRootProps = $props();

	useStepperRoot({
		step: box.with(
			() => step,
			(v) => (step = v)
		)
	});
</script>

{@render children?.()}
