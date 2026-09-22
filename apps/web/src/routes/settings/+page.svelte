<script lang="ts">
	import { page } from '$app/state';
	import SettingsView from '$lib/features/settings/SettingsView.svelte';
	import { collapseSettings } from '$lib/features/settings/navigation';

	const canCollapseToModal = $derived(Boolean(page.state.settingsExpansion));
</script>

<div
	class="min-h-svh"
	style="view-transition-name: settings-panel;"
	data-testid="settings-page-transition"
>
	<SettingsView
		presentation="page"
		oncollapse={canCollapseToModal ? collapseSettings : undefined}
	/>
</div>
