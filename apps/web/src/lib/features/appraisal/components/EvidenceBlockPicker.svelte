<script lang="ts">
	import type { DocumentBlockDto } from '$lib/api/generated/models';
	import { Button } from '@deepref/ui/button';
	import { Input } from '@deepref/ui/input';
	import { Plus } from '@lucide/svelte';
	import { blockSnippet, searchEvidenceBlocks } from '../evidence-search';

	type Props = {
		/** Prefix for element ids and test ids, so several pickers can share a page. */
		idPrefix: string;
		questionLabel: string;
		blocks: DocumentBlockDto[];
		/** Blocks already cited for this question; they are not offered again. */
		excludeIds?: readonly string[];
		onSelect: (block: DocumentBlockDto) => void;
	};

	let { idPrefix, questionLabel, blocks, excludeIds = [], onSelect }: Props = $props();

	const RESULT_LIMIT = 8;
	let open = $state(false);
	let query = $state('');
	const candidates = $derived(blocks.filter((block) => !excludeIds.includes(block.id)));
	const matches = $derived(searchEvidenceBlocks(candidates, query, RESULT_LIMIT));

	function close(): void {
		open = false;
		query = '';
	}

	function choose(block: DocumentBlockDto): void {
		onSelect(block);
		close();
	}
</script>

{#if open}
	<div
		class="flex min-w-0 flex-col gap-2 rounded-lg border border-border/80 bg-background p-3"
		data-testid={`${idPrefix}-picker`}
	>
		<label for={`${idPrefix}-search`} class="text-xs font-medium"
			>Search this report's text</label
		>
		<Input
			id={`${idPrefix}-search`}
			type="search"
			autocomplete="off"
			placeholder="Words from the passage, or p4 for page 4"
			bind:value={query}
		/>
		<p class="text-xs text-muted-foreground" aria-live="polite">
			{query.trim()
				? `${matches.total} matching block${matches.total === 1 ? '' : 's'}`
				: `Showing the first ${matches.results.length} of ${matches.total} blocks`}
		</p>
		<ul
			class="flex max-h-72 flex-col gap-1 overflow-y-auto"
			aria-label={`Blocks for ${questionLabel}`}
		>
			{#each matches.results as block (block.id)}
				<li>
					<button
						type="button"
						class="flex w-full min-w-0 flex-col gap-0.5 rounded-md border border-transparent px-2 py-1.5 text-left text-xs outline-none hover:border-border hover:bg-muted/40 focus-visible:ring-3 focus-visible:ring-ring/30"
						onclick={() => choose(block)}
						data-testid={`${idPrefix}-option`}
					>
						<span class="font-medium text-muted-foreground">p. {block.page_number}</span
						>
						<span class="break-words">{blockSnippet(block.text)}</span>
					</button>
				</li>
			{:else}
				<li class="px-2 py-1 text-xs text-muted-foreground">
					No parsed block matches “{query.trim()}”.
				</li>
			{/each}
		</ul>
		<div class="flex justify-end">
			<Button type="button" variant="ghost" size="sm" onclick={close}>Close</Button>
		</div>
	</div>
{:else}
	<Button
		type="button"
		variant="outline"
		size="sm"
		class="self-start"
		aria-expanded={open}
		data-testid={`${idPrefix}-open`}
		onclick={() => (open = true)}
	>
		<Plus aria-hidden="true" data-icon="inline-start" />Add evidence block
	</Button>
{/if}
