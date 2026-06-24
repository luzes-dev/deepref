<script lang="ts">
	import { resolve } from '$app/paths';
	import { Button } from '$lib/components/ui/button';
	import NetworkIcon from '@lucide/svelte/icons/network';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import SettingsIcon from '@lucide/svelte/icons/settings';
</script>

<header class="border-b bg-background">
	<div class="mx-auto flex max-w-7xl items-center justify-between gap-4 px-4 py-3">
		<a href={resolve('/projects')} class="flex items-center gap-2 font-semibold">
			<NetworkIcon />
			DeepRef
		</a>
		<nav class="flex items-center gap-2">
			<Button variant="ghost" size="sm" href={resolve('/projects')}>Projects</Button>
			<Button variant="ghost" size="sm" href={resolve('/ingestions')}>Ingestions</Button>
			<Button variant="ghost" size="sm" href={resolve('/system')}>System</Button>
			<Button variant="outline" size="sm" href={resolve('/settings')}>
				<SettingsIcon data-icon="inline-start" />
				Settings
			</Button>
			<Button size="sm" href={resolve('/ingestions')}>
				<PlusIcon data-icon="inline-start" />
				Ingest
			</Button>
		</nav>
	</div>
</header>
