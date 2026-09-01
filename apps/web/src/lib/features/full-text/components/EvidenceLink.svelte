<script lang="ts">
	import type { DocumentBlockDto } from '$lib/api/generated/models';
	import { Badge } from '$lib/components/ui/badge';
	import { FileSearch } from '@lucide/svelte';

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
	class="group flex min-h-16 w-full items-start gap-3 rounded-lg border bg-card p-3 text-left text-sm transition-colors hover:bg-muted/50 focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none {selected
		? 'border-primary bg-primary/10 shadow-[inset_3px_0_0_var(--primary)]'
		: 'border-border/70'}"
	aria-pressed={selected}
	onclick={() => onSelect(block)}
>
	<span
		class="flex size-7 shrink-0 items-center justify-center rounded-md bg-primary/10 text-primary"
	>
		<FileSearch aria-hidden="true" />
	</span>
	<span class="min-w-0 flex-1">
		<span class="flex flex-wrap items-center gap-2">
			<Badge variant={selected ? 'default' : 'outline'}>Page {block.page_number}</Badge>
			<span class="text-[11px] tracking-wide text-muted-foreground uppercase"
				>{block.kind}</span
			>
		</span>
		<span class="mt-2 block leading-6 text-foreground/90">{block.text}</span>
	</span>
</button>
