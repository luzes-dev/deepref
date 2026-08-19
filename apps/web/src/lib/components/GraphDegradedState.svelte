<script lang="ts">
	import * as Alert from '$lib/components/ui/alert';
	import { Button } from '$lib/components/ui/button';
	import type { ProjectionMetadata, ProjectionStatusDto } from '$lib/api/generated/models';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
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
</script>

<Alert.Root variant="destructive" data-testid="graph-degraded-state">
	<CircleAlertIcon />
	<Alert.Title>
		{feature} request failed
	</Alert.Title>
	<Alert.Description>
		<p>
			{error instanceof Error
				? error.message
				: 'An unexpected error prevented this graph request.'}
		</p>
		{#if projection}
			<p class="mt-2">Graph metrics revision {projection.revision}; lag {projection.lag}.</p>
		{/if}
	</Alert.Description>
	<Alert.Action>
		<Button variant="outline" size="sm" onclick={onRetry}>
			<RefreshCwIcon data-icon="inline-start" />Retry {feature.toLowerCase()}
		</Button>
	</Alert.Action>
</Alert.Root>
