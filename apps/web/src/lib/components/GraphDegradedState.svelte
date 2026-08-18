<script lang="ts">
	import * as Alert from '$lib/components/ui/alert';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import type { ProjectionMetadata, ProjectionStatusDto } from '$lib/api/generated/models';
	import { ApiError } from '$lib/api/custom-fetch';
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

	const apiError = $derived(error instanceof ApiError ? error : undefined);
	const graphUnavailable = $derived(apiError?.code === 'GRAPH_UNAVAILABLE');
	const retryLabel = $derived(
		apiError?.retryAfterSeconds === undefined
			? undefined
			: `${apiError.retryAfterSeconds} seconds`
	);
</script>

<Alert.Root
	variant={graphUnavailable ? 'default' : 'destructive'}
	data-testid="graph-degraded-state"
>
	<CircleAlertIcon />
	<Alert.Title>
		{graphUnavailable ? `${feature} temporarily unavailable` : `${feature} request failed`}
	</Alert.Title>
	<Alert.Description>
		<p>
			{graphUnavailable
				? 'The graph read model is recovering. Core project, article, and ingestion features are still usable.'
				: error instanceof Error
					? error.message
					: 'An unexpected error prevented this graph request.'}
		</p>
		<div class="mt-2 flex flex-wrap gap-2">
			{#if apiError?.code}<Badge variant="outline">Code {apiError.code}</Badge>{/if}
			{#if retryLabel}<Badge variant="secondary">Retry after {retryLabel}</Badge>{/if}
			{#if apiError?.correlationId}
				<Badge variant="outline">Correlation {apiError.correlationId}</Badge>
			{/if}
			{#if apiError?.requestId}<Badge variant="outline">Request {apiError.requestId}</Badge
				>{/if}
			{#if projection}
				<Badge variant="secondary">Projection revision {projection.revision}</Badge>
				<Badge variant="outline">Lag {projection.lag}</Badge>
			{/if}
		</div>
	</Alert.Description>
	<Alert.Action>
		<Button variant="outline" size="sm" onclick={onRetry}>
			<RefreshCwIcon data-icon="inline-start" />Retry {feature.toLowerCase()}
		</Button>
	</Alert.Action>
</Alert.Root>
