<script lang="ts">
	import { useQueryClient } from '@tanstack/svelte-query';
	import { Button } from '@deepref/ui/button';
	import { createGetCurrentUser, createLogout } from '#lib/api/generated/auth/auth.js';
	import { SIGN_IN_PATH } from './auth-urls.js';
	import { sessionStatus } from './session-status.js';

	const queryClient = useQueryClient();
	const me = createGetCurrentUser(() => ({ query: { retry: false, staleTime: 60_000 } }));
	const logout = createLogout();
	const status = $derived(sessionStatus(me));

	async function signOut() {
		try {
			await logout.mutateAsync();
		} finally {
			// Drop every cached answer that belonged to the signed-out person.
			queryClient.clear();
			window.location.assign(SIGN_IN_PATH);
		}
	}
</script>

{#if status.kind === 'signed-in'}
	<div class="flex items-center gap-2" data-testid="user-menu">
		<span class="max-w-40 truncate text-xs text-muted-foreground" title={status.user.email}>
			{status.user.display_name ?? status.user.email}
		</span>
		<Button variant="ghost" size="xs" onclick={signOut} disabled={logout.isPending}>
			Sign out
		</Button>
	</div>
{/if}
