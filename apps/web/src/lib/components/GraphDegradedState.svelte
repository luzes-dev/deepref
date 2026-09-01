<script lang="ts">
	import { StatePanel } from '$lib/components/layout';
	import { Button } from '$lib/components/ui/button';
	import type { ProjectionMetadata, ProjectionStatusDto } from '$lib/api/generated/models';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';

	let {
		error,
		feature = 'Graph',
		projection,
		onRetry
	}: {
		error: unknown;
		feature?: string;
		projection?: ProjectionMetadata | ProjectionStatusDto;
		onRetry: () => void;
	} = $props();

	const description = $derived.by(() => {
		const message =
			error instanceof Error
				? error.message
				: 'An unexpected error prevented this graph request.';
		const projectionDetails = projection
			? ` ${feature} metrics revision ${projection.revision}; lag ${projection.lag}.`
			: '';
		return `${message}${projectionDetails}`;
	});
</script>

<StatePanel
	testId="graph-degraded-state"
	state="degraded"
	status="Degraded"
	title={`${feature} request failed`}
	{description}
>
	{#snippet action()}
		<Button variant="outline" size="sm" onclick={onRetry}>
			<RefreshCwIcon data-icon="inline-start" />Retry {feature.toLowerCase()}
		</Button>
	{/snippet}
</StatePanel>
