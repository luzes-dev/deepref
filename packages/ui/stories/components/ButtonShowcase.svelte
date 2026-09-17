<script lang="ts">
	import { Button } from '../../src/primitives/button';
	import { LoadingButton } from '../../src/patterns/loading-button';
	import Download from '@lucide/svelte/icons/download';
	import Plus from '@lucide/svelte/icons/plus';
	let pending = $state(false);
</script>
<div class="story-sheet">
	<header><h1>Actions</h1><p>One primary action per task. Use outline for alternatives, ghost for adjacent tools, and destructive for removal.</p></header>
	<section><h2>Hierarchy</h2><div class="flex flex-wrap gap-2">
		<Button>Save changes</Button><Button variant="outline">Preview</Button><Button variant="secondary">Duplicate</Button><Button variant="ghost">Dismiss</Button><Button variant="destructive">Delete record</Button><Button variant="link" href="#sizes">View sizes</Button>
	</div></section>
	<section id="sizes"><h2>Density</h2><p>Default controls are 36px. Use 32px in toolbars and 24px only for compact, well-spaced actions.</p><div class="flex flex-wrap items-end gap-3">
		<Button size="xs">Compact</Button><Button size="sm">Toolbar</Button><Button>Default</Button><Button size="lg">Prominent</Button>
	</div></section>
	<section><h2>Icons and accessible names</h2><div class="flex flex-wrap gap-2"><Button variant="outline"><Download data-icon="inline-start" />Export records</Button><Button variant="outline" size="icon" aria-label="Add record"><Plus /></Button></div></section>
	<section><h2>Unavailable and pending</h2><div class="flex flex-wrap gap-2"><Button disabled>Save changes</Button><Button variant="outline" disabled>Preview</Button><Button variant="outline" href="#sizes" disabled>Unavailable link</Button><LoadingButton loading>Saving changes</LoadingButton></div></section>
	<section><h2>Loading contract</h2><p>The label stays visible while the action is unavailable. Busy state is announced without adding a second spinner name.</p><div class="flex flex-wrap gap-2"><LoadingButton loading={pending} onclick={() => pending = true}>Run export</LoadingButton><Button variant="ghost" onclick={() => pending = false}>Reset example</Button></div></section>
</div>
