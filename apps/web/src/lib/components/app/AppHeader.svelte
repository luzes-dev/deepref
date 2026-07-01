<script lang="ts">
	import { resolve } from '$app/paths';
	import { Button } from '$lib/components/ui/button';
	import NetworkIcon from '@lucide/svelte/icons/network';
	import SettingsIcon from '@lucide/svelte/icons/settings';
</script>

<header class="border-b bg-background">
	<div class="mx-auto flex max-w-7xl items-center justify-between gap-4 px-4 py-3">
		<a href={resolve('/')} class="flex items-center gap-2 font-semibold">
			<NetworkIcon />
			DeepRef
		</a>
		<nav class="flex items-center gap-2">
			<Button variant="ghost" size="sm" href={resolve('/')}>Projects</Button>
			<Button variant="outline" size="sm" href={resolve('/settings')}>
				<SettingsIcon data-icon="inline-start" />
				Settings
			</Button>
		</nav>
	</div>
</header>
