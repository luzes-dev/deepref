<script lang="ts">
	import * as Alert from '$lib/components/ui/alert';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import type { DependencyStatus } from '$lib/api/generated/models';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';

	let {
		status,
		error,
		isFetching = false,
		onRetry
	}: {
		status?: DependencyStatus;
		error?: Error | null;
		isFetching?: boolean;
		onRetry: () => void;
	} = $props();

	const dependencies = $derived(
		status ? Object.entries(status).filter(([, detail]) => detail.state !== 'available') : []
	);
	const coreUnavailable = $derived(status?.postgresql.state === 'unavailable');
</script>

{#if error}
	<Alert.Root variant="destructive" data-testid="dependency-banner">
		<CircleAlertIcon />
		<Alert.Title>Dependency status unavailable</Alert.Title>
		<Alert.Description>
			The workspace will keep using the last known service state. {error.message}
		</Alert.Description>
		<Alert.Action>
			<Button variant="outline" size="sm" onclick={onRetry} disabled={isFetching}>
				<RefreshCwIcon data-icon="inline-start" />Refresh
			</Button>
		</Alert.Action>
	</Alert.Root>
{:else if dependencies.length > 0}
	<Alert.Root
		variant={coreUnavailable ? 'destructive' : 'default'}
		data-testid="dependency-banner"
	>
		<CircleAlertIcon />
		<Alert.Title
			>{coreUnavailable
				? 'Core service interruption'
				: 'Some features are degraded'}</Alert.Title
		>
		<Alert.Description>
			<div class="flex flex-wrap items-center gap-2">
				{#each dependencies as [name, detail] (name)}
					<Badge variant={detail.state === 'unavailable' ? 'destructive' : 'secondary'}>
						{name}: {detail.state}
						{#if detail.lag}
							· lag {detail.lag}{/if}
						{#if detail.backlog}
							· backlog {detail.backlog}{/if}
					</Badge>
				{/each}
			</div>
			{#if !coreUnavailable}
				<p class="mt-2">
					Projects, articles, and ingestions remain available while durable jobs drain.
				</p>
			{/if}
		</Alert.Description>
		<Alert.Action>
			<Button variant="outline" size="sm" onclick={onRetry} disabled={isFetching}>
				<RefreshCwIcon data-icon="inline-start" />Refresh
			</Button>
		</Alert.Action>
	</Alert.Root>
{/if}
