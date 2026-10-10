<script lang="ts">
	import type { DocumentBlockDto } from '#lib/api/generated/models/index.js';
	import { Button } from '@deepref/ui/button';
	import { X } from '@lucide/svelte';

	let {
		block,
		onClear
	}: {
		/** The evidence block named by the `block` URL parameter. */
		block: DocumentBlockDto;
		onClear: () => void;
	} = $props();
</script>

<section
	class="flex items-start gap-3 rounded-lg border border-primary/30 bg-primary/5 p-3 text-sm"
	aria-label="Cited passage"
	data-testid="cited-passage-banner"
>
	<div class="min-w-0 flex-1">
		<p class="text-2xs font-semibold tracking-widest-caps text-primary uppercase">
			Cited passage · page {block.page_number}
		</p>
		<p class="mt-1 max-h-32 overflow-y-auto leading-6 break-words text-foreground/90">
			{block.text}
		</p>
	</div>
	<Button variant="ghost" size="icon-xs" aria-label="Clear cited passage" onclick={onClear}
		><X aria-hidden="true" /></Button
	>
</section>
