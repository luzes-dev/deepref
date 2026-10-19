<script lang="ts">
	import type { Snippet } from 'svelte';
	import { page } from '$app/state';
	import { Spinner } from '@deepref/ui/spinner';
	import { createGetCurrentUser } from '#lib/api/generated/auth/auth.js';
	import SignInScreen from './SignInScreen.svelte';
	import { sessionStatus } from './session-status.js';

	let { children }: { children: Snippet } = $props();

	// The server decides who may read what. This gate only chooses what to show.
	const me = createGetCurrentUser(() => ({
		query: { retry: false, staleTime: 60_000, refetchOnWindowFocus: true }
	}));
	const status = $derived(sessionStatus(me));
</script>

{#if status.kind === 'loading'}
	<div class="flex min-h-svh items-center justify-center" role="status" aria-label="Loading">
		<Spinner />
	</div>
{:else if status.kind === 'signed-in' || status.kind === 'disabled'}
	{@render children()}
{:else}
	<SignInScreen {status} errorCode={page.url.searchParams.get('error')} location={page.url} />
{/if}
