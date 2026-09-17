<script lang="ts">
	import { Button } from '../../src/primitives/button';
	import { Badge } from '../../src/primitives/badge';
	import * as Alert from '../../src/primitives/alert';
	import Check from '@lucide/svelte/icons/check';
	import TriangleAlert from '@lucide/svelte/icons/triangle-alert';
	import CircleAlert from '@lucide/svelte/icons/circle-alert';
	import Info from '@lucide/svelte/icons/info';
</script>
<div class="story-sheet">
	<header><h1>Status and feedback</h1><p>Status uses a readable foreground, tinted surface, and related border. Always include a label; color alone carries no contract.</p></header>
	<section><h2>Dense status</h2><div class="flex flex-wrap gap-2"><Badge variant="success"><Check />Complete</Badge><Badge variant="warning"><TriangleAlert />Needs attention</Badge><Badge variant="destructive"><CircleAlert />Failed</Badge><Badge variant="info"><Info />In progress</Badge><Badge variant="secondary">Draft</Badge><Badge variant="outline">Archived</Badge></div></section>
	<section class="max-w-2xl"><h2>Contextual feedback</h2><div class="flex flex-col gap-3">
		<Alert.Root variant="success"><Check /><Alert.Title>Export complete</Alert.Title><Alert.Description>All 248 records are available for download.</Alert.Description></Alert.Root>
		<Alert.Root variant="warning"><TriangleAlert /><Alert.Title>Some records need attention</Alert.Title><Alert.Description>Three records have incomplete metadata. You can continue, but review the missing values before sharing this export.</Alert.Description></Alert.Root>
		<Alert.Root variant="destructive"><CircleAlert /><Alert.Title>Changes could not be saved</Alert.Title><Alert.Description>Your entries are still available. Check your connection and try again.</Alert.Description></Alert.Root>
		<Alert.Root variant="info"><Info /><Alert.Title>Export is running</Alert.Title><Alert.Description>You can continue working while the file is prepared.</Alert.Description></Alert.Root>
	</div></section>
	<section class="max-w-sm"><h2>Action with a long title</h2><Alert.Root><Info /><Alert.Title>This published version is retained for the collection history</Alert.Title><Alert.Description>Create a new version to make changes while preserving the existing record.</Alert.Description><Alert.Action><Button variant="outline">Create an amended version</Button></Alert.Action></Alert.Root></section>
	<section><h2>Long labels</h2><div class="max-w-60"><Badge variant="warning">Waiting for confirmation from another reviewer before proceeding</Badge></div></section>
</div>
