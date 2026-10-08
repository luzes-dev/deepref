<script lang="ts">
	import { parseAssistantMarkdown, type MarkdownNode, type MarkdownTag } from '../markdown';

	let { source }: { source: string } = $props();

	const nodes = $derived(parseAssistantMarkdown(source));

	const CODE = 'rounded bg-muted px-1 py-0.5 font-mono text-xs text-foreground';
	const CODE_BLOCK = 'font-mono text-xs text-foreground';

	const CLASS_BY_TAG: Record<MarkdownTag, string> = {
		p: 'my-2 first:mt-0 last:mb-0',
		h1: 'mt-3 mb-1 text-base font-semibold text-foreground first:mt-0',
		h2: 'mt-3 mb-1 text-base font-semibold text-foreground first:mt-0',
		h3: 'mt-3 mb-1 font-semibold text-foreground first:mt-0',
		h4: 'mt-2 mb-1 font-semibold text-foreground first:mt-0',
		h5: 'mt-2 mb-1 font-semibold text-foreground first:mt-0',
		h6: 'mt-2 mb-1 font-semibold text-muted-foreground first:mt-0',
		strong: 'font-semibold text-foreground',
		em: 'italic',
		del: 'text-muted-foreground line-through',
		code: CODE,
		pre: 'my-2 overflow-x-auto rounded-md bg-muted p-3',
		a: 'font-medium break-words text-primary underline underline-offset-2 hover:text-primary/80',
		ul: 'my-2 list-disc space-y-1 pl-5 marker:text-muted-foreground',
		ol: 'my-2 list-decimal space-y-1 pl-5 marker:text-muted-foreground',
		li: 'pl-1',
		blockquote: 'my-2 border-l-2 border-border pl-3 text-muted-foreground',
		hr: 'my-3 border-border',
		table: 'my-2 block max-w-full overflow-x-auto border-collapse text-xs',
		thead: '',
		tbody: '',
		tr: 'border-b border-border last:border-b-0',
		th: 'border border-border bg-muted px-2 py-1 text-left align-top font-semibold text-foreground',
		td: 'border border-border px-2 py-1 align-top'
	};

	function classFor(node: Extract<MarkdownNode, { kind: 'element' }>): string {
		if (node.tag === 'code' && node.block) return CODE_BLOCK;
		return CLASS_BY_TAG[node.tag];
	}
</script>

{#snippet renderNodes(list: MarkdownNode[])}
	{#each list as node, index (index)}
		{#if node.kind === 'text'}
			{node.text}
		{:else if node.kind === 'break'}
			<br />
		{:else}
			<svelte:element
				this={node.tag}
				class={classFor(node)}
				href={node.href}
				target={node.external ? '_blank' : undefined}
				rel={node.external ? 'noopener noreferrer' : undefined}
				start={node.start}
			>
				{@render renderNodes(node.children)}
			</svelte:element>
		{/if}
	{/each}
{/snippet}

<div class="min-w-0 text-sm leading-relaxed break-words" data-testid="assistant-markdown">
	{@render renderNodes(nodes)}
</div>
