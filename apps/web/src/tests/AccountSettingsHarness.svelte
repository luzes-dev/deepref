<script lang="ts">
	import { QueryClient, QueryClientProvider } from '@tanstack/svelte-query';
	import ApiKeySettings from '#lib/features/settings/ApiKeySettings.svelte';
	import BillingSettings from '#lib/features/settings/BillingSettings.svelte';
	import AccountSettings from '#lib/features/settings/AccountSettings.svelte';
	import SettingsView from '#lib/features/settings/SettingsView.svelte';
	import PrivacySettings from '#lib/features/settings/PrivacySettings.svelte';
	let {
		client,
		view,
		workspaceId = 'workspace-a',
		navigate
	}: {
		client: QueryClient;
		view: 'keys' | 'billing' | 'privacy' | 'account' | 'settings';
		workspaceId?: string;
		navigate?: (url: string) => void;
	} = $props();
</script>

<QueryClientProvider {client}>
	{#key workspaceId}
		{#if view === 'keys'}<ApiKeySettings
				{workspaceId}
			/>{:else if view === 'billing'}<BillingSettings
				{workspaceId}
				{navigate}
			/>{:else if view === 'account'}<AccountSettings
			/>{:else if view === 'settings'}<SettingsView />{:else}<PrivacySettings />{/if}
	{/key}
</QueryClientProvider>
