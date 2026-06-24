<script lang="ts">
	import * as Card from '$lib/components/ui/card';
	import { Badge } from '$lib/components/ui/badge';
	import { createGetHealth } from '$lib/api/generated/health/health';

	const apiHealth = createGetHealth();
	const health = $derived(
		apiHealth.isError ? 'failed' : apiHealth.data?.data.status || 'checking'
	);
</script>

<div class="grid gap-4 md:grid-cols-3">
	<Card.Root>
		<Card.Header
			><Card.Title>API</Card.Title><Card.Description>Rust Axum service</Card.Description
			></Card.Header
		>
		<Card.Content
			><Badge
				variant={health === 'ok'
					? 'default'
					: health === 'failed'
						? 'destructive'
						: 'secondary'}>{health}</Badge
			></Card.Content
		>
	</Card.Root>
	<Card.Root>
		<Card.Header
			><Card.Title>NATS</Card.Title><Card.Description
				>JetStream-compatible subjects</Card.Description
			></Card.Header
		>
		<Card.Content><Badge variant="secondary">Configured</Badge></Card.Content>
	</Card.Root>
	<Card.Root>
		<Card.Header
			><Card.Title>Neo4j</Card.Title><Card.Description
				>Graph constraints in infra</Card.Description
			></Card.Header
		>
		<Card.Content><Badge variant="outline">Ready</Badge></Card.Content>
	</Card.Root>
	{#if apiHealth.error}
		<Card.Root class="md:col-span-3"
			><Card.Content class="p-6 text-sm text-muted-foreground"
				>{apiHealth.error.message}</Card.Content
			></Card.Root
		>
	{/if}
</div>
