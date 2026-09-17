<script lang="ts">
	import { Button } from '../../src/primitives/button';
	import { Input } from '../../src/primitives/input';
	import * as Field from '../../src/primitives/field';
	import * as Dialog from '../../src/primitives/dialog';
	import * as Sheet from '../../src/primitives/sheet';
	import * as Drawer from '../../src/primitives/drawer';
	import * as Menu from '../../src/primitives/dropdown-menu';
	import * as Tooltip from '../../src/primitives/tooltip';
	import * as Popover from '../../src/primitives/popover';
	import Info from '@lucide/svelte/icons/info';
	let { longContent = false } = $props<{ longContent?: boolean }>();
	let result = $state('No action selected');
</script>
<Tooltip.Provider>
	<div class="story-sheet">
		<header><h1>Overlays</h1><p>Dialogs interrupt a task; sheets retain context beside it. Every overlay has a name, a reachable close control, and a keyboard route back to its trigger.</p></header>
		<div class="flex flex-wrap gap-2">
			<Dialog.Root><Dialog.Trigger>{#snippet child({ props })}<Button {...props}>Edit record</Button>{/snippet}</Dialog.Trigger><Dialog.Content><Dialog.Header><Dialog.Title>Edit record details</Dialog.Title><Dialog.Description>Changes apply to this record only. Review the name before saving.</Dialog.Description></Dialog.Header><Field.Group><Field.Field><Field.Label for="dialog-name">Record name</Field.Label><Input id="dialog-name" value="Reference collection" /></Field.Field>{#if longContent}{#each Array.from({length: 8}, (_, i) => i + 1) as number (number)}<Field.Field><Field.Label for={`detail-${number}`}>Additional detail {number}</Field.Label><Input id={`detail-${number}`} value="A deliberately long form that must remain reachable in a short viewport" /></Field.Field>{/each}{/if}</Field.Group><Dialog.Footer><Dialog.Close>{#snippet child({props})}<Button {...props} variant="outline">Cancel</Button>{/snippet}</Dialog.Close><Dialog.Close>{#snippet child({props})}<Button {...props}>Save changes</Button>{/snippet}</Dialog.Close></Dialog.Footer></Dialog.Content></Dialog.Root>
			<Sheet.Root><Sheet.Trigger>{#snippet child({props})}<Button {...props} variant="outline">Open inspector</Button>{/snippet}</Sheet.Trigger><Sheet.Content><Sheet.Header><Sheet.Title>Record inspector</Sheet.Title><Sheet.Description>Supporting details remain readable on a narrow screen.</Sheet.Description></Sheet.Header><div class="flex flex-col gap-4 px-4">{#each ['Identifier', 'Owner', 'Last updated', 'Source', 'Version', 'Availability'] as label (label)}<div><h3 class="text-xs text-muted-foreground">{label}</h3><p class="mt-1 text-sm">Reference collection · shared workspace</p></div>{/each}</div></Sheet.Content></Sheet.Root>
			<Drawer.Root><Drawer.Trigger>{#snippet child({props})}<Button {...props} variant="outline">Open drawer</Button>{/snippet}</Drawer.Trigger><Drawer.Content><div class="mx-auto w-full max-w-lg"><Drawer.Header><Drawer.Title>Export options</Drawer.Title><Drawer.Description>Choose a format that preserves the information you need.</Drawer.Description></Drawer.Header><p class="px-4 text-sm text-muted-foreground">CSV includes visible fields. JSON includes all structured metadata.</p><Drawer.Footer><Drawer.Close>{#snippet child({props})}<Button {...props} variant="outline">Done</Button>{/snippet}</Drawer.Close></Drawer.Footer></div></Drawer.Content></Drawer.Root>
			<Menu.Root><Menu.Trigger>{#snippet child({props})}<Button {...props} variant="outline">Record actions</Button>{/snippet}</Menu.Trigger><Menu.Content><Menu.Group><Menu.GroupHeading>Record actions</Menu.GroupHeading><Menu.Item onSelect={() => result = 'Duplicate selected'}>Duplicate</Menu.Item><Menu.Item disabled>Archive</Menu.Item><Menu.Separator /><Menu.Item variant="destructive" onSelect={() => result = 'Delete selected'}>Delete</Menu.Item></Menu.Group></Menu.Content></Menu.Root>
			<Popover.Root><Popover.Trigger>{#snippet child({props})}<Button {...props} variant="ghost">About exports</Button>{/snippet}</Popover.Trigger><Popover.Content><h2 class="text-sm font-semibold">Portable records</h2><p class="mt-2 text-sm text-muted-foreground">Exported files include stable identifiers so you can trace each row to its source.</p></Popover.Content></Popover.Root>
			<Tooltip.Root><Tooltip.Trigger>{#snippet child({props})}<Button {...props} variant="ghost" size="icon" aria-label="Keyboard help"><Info /></Button>{/snippet}</Tooltip.Trigger><Tooltip.Content>Press Escape to close an overlay.</Tooltip.Content></Tooltip.Root>
		</div>
		<p role="status">{result}</p>
	</div>
</Tooltip.Provider>
