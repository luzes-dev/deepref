<script lang="ts">
	import * as Sidebar from '../../src/primitives/sidebar';
	import * as Empty from '../../src/primitives/empty';
	import * as Modal from '../../src/primitives/modal';
	import { PageFrame } from '../../src/patterns/page-frame';
	import { PageHeader } from '../../src/patterns/page-header';
	import { Button } from '../../src/primitives/button';
	import { Toaster } from '../../src/primitives/sonner';
	import { toast } from 'svelte-sonner';
	import Inbox from '@lucide/svelte/icons/inbox';
	import Archive from '@lucide/svelte/icons/archive';
	let active = $state('Collection');
	let open = $state(false);
</script>
<PageFrame>
	<Sidebar.Provider>
		<Sidebar.Root collapsible="icon"><Sidebar.Header><span class="px-2 py-1 text-sm font-semibold">Workspace</span></Sidebar.Header><Sidebar.Content><Sidebar.Group><Sidebar.GroupLabel>Library</Sidebar.GroupLabel><Sidebar.Menu>{#each ['Collection', 'Archive'] as label (label)}<Sidebar.MenuItem><Sidebar.MenuButton isActive={active === label} onclick={() => active = label} tooltipContent={label}>{#if label === 'Collection'}<Inbox />{:else}<Archive />{/if}<span>{label}</span></Sidebar.MenuButton></Sidebar.MenuItem>{/each}</Sidebar.Menu></Sidebar.Group></Sidebar.Content><Sidebar.Footer><span class="px-2 text-xs text-muted-foreground">Use the Storybook theme toolbar</span></Sidebar.Footer></Sidebar.Root>
		<Sidebar.Inset><div class="flex items-center gap-2 border-b p-3"><Sidebar.Trigger /><span class="text-sm text-muted-foreground">{active}</span></div><div class="flex flex-col gap-6 p-page"><PageHeader title={active} description="The sidebar collapses to an icon rail on desktop and opens as a sheet on narrow screens." /><Empty.Root><Empty.Header><Empty.Media variant="icon"><Inbox /></Empty.Media><Empty.Title>No records yet</Empty.Title><Empty.Description>Add a record to begin organizing this collection.</Empty.Description></Empty.Header><Empty.Content><Button onclick={() => open = true}>Add a record</Button></Empty.Content></Empty.Root></div></Sidebar.Inset>
	</Sidebar.Provider>
</PageFrame>
<Modal.Root bind:open><Modal.Content><Modal.Header><Modal.Title>A responsive modal</Modal.Title><Modal.Description>A dialog on desktop and a drawer on narrow screens. Content and actions have the same meaning.</Modal.Description></Modal.Header><Modal.Footer><Button variant="outline" onclick={() => open = false}>Cancel</Button><Button onclick={() => { open = false; toast.success('Example record added'); }}>Confirm example</Button></Modal.Footer></Modal.Content></Modal.Root>
<Toaster />
