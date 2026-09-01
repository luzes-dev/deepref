<script lang="ts">
	import * as Alert from '$lib/components/ui/alert';
	import { Info } from '@lucide/svelte';

	let {
		errorTitle,
		errorMessage,
		statusMessage
	}: {
		errorTitle: string;
		errorMessage?: string;
		statusMessage?: string;
	} = $props();
</script>

{#if errorMessage}
	<Alert.Root variant="destructive" role="alert">
		<Info aria-hidden="true" />
		<Alert.Title>{errorTitle}</Alert.Title>
		<Alert.Description>{errorMessage}</Alert.Description>
	</Alert.Root>
{/if}
{#if statusMessage}
	<div
		class="flex items-start gap-2 rounded-lg border border-warning/40 bg-warning/10 p-3 text-sm"
		role="status"
		aria-live="polite"
	>
		<Info class="mt-0.5 shrink-0 text-warning" aria-hidden="true" />
		<p>{statusMessage}</p>
	</div>
{/if}
