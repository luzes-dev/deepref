<script lang="ts">
	import * as Tabs from '../../src/primitives/tabs';
	import * as Breadcrumb from '../../src/primitives/breadcrumb';
	import * as Command from '../../src/primitives/command';
	import { Button } from '../../src/primitives/button';
	import { Toggle } from '../../src/primitives/toggle';
	import * as ToggleGroup from '../../src/primitives/toggle-group';
	let open = $state(false);
	let selected = $state('No command selected');
	let view = $state('list');
</script>
<div class="story-sheet">
	<header><h1>Navigation and selection</h1><p>Tabs switch related panels. Toggle groups choose a view. Commands select an action with search and keyboard navigation.</p></header>
	<section><h2>Location</h2><Breadcrumb.Root><Breadcrumb.List><Breadcrumb.Item><Breadcrumb.Link href="#panels">Collection</Breadcrumb.Link></Breadcrumb.Item><Breadcrumb.Separator /><Breadcrumb.Item><Breadcrumb.Page>Record details</Breadcrumb.Page></Breadcrumb.Item></Breadcrumb.List></Breadcrumb.Root></section>
	<section id="panels"><h2>Related panels</h2><Tabs.Root value="details"><Tabs.List aria-label="Record panels"><Tabs.Trigger value="details">Details</Tabs.Trigger><Tabs.Trigger value="history">History and activity</Tabs.Trigger><Tabs.Trigger value="attachments">Attachments</Tabs.Trigger><Tabs.Trigger value="restricted" disabled>Restricted</Tabs.Trigger></Tabs.List><Tabs.Content value="details"><p class="py-3 text-sm">Record details and descriptive metadata.</p></Tabs.Content><Tabs.Content value="history"><p class="py-3 text-sm">There are no changes to show yet.</p></Tabs.Content><Tabs.Content value="attachments"><p class="py-3 text-sm">No attachments have been added.</p></Tabs.Content></Tabs.Root></section>
	<section><h2>View preferences</h2><div class="flex flex-wrap gap-3"><ToggleGroup.Root type="single" bind:value={view} aria-label="Collection view"><ToggleGroup.Item value="list">List</ToggleGroup.Item><ToggleGroup.Item value="grid">Grid</ToggleGroup.Item></ToggleGroup.Root><Toggle aria-label="Show archived records">Show archived</Toggle></div></section>
	<section><h2>Command search</h2><Button variant="outline" onclick={() => open = true}>Find an action</Button><p role="status">{selected}</p></section>
	<Command.Dialog bind:open title="Find an action" description="Search available record actions." showCloseButton><Command.Input placeholder="Search actions" aria-label="Search actions" /><Command.List><Command.Empty>No actions match your search.</Command.Empty><Command.Group heading="Records"><Command.Item value="new" onSelect={() => { selected = 'New record selected'; open = false; }}>New record</Command.Item><Command.Item value="export" onSelect={() => { selected = 'Export selected'; open = false; }}>Export collection</Command.Item><Command.Item value="archive" disabled>Archive collection</Command.Item></Command.Group></Command.List></Command.Dialog>
</div>
