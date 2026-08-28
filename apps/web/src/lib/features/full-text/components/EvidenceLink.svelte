<script lang="ts">
	import type { DocumentBlockDto } from '$lib/api/generated/models';

	let {
		block,
		selected = false,
		onSelect
	}: {
		block: DocumentBlockDto;
		selected?: boolean;
		onSelect: (block: DocumentBlockDto) => void;
	} = $props();
</script>

<button
	type="button"
	class="w-full rounded-md border p-3 text-left text-sm transition-colors hover:bg-muted/50 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none {selected
		? 'border-primary bg-primary/5'
		: ''}"
	aria-pressed={selected}
	onclick={() => onSelect(block)}
>
	<span class="mr-2 text-xs text-muted-foreground">p. {block.page_number} · {block.kind}</span>
	{block.text}
</button>
