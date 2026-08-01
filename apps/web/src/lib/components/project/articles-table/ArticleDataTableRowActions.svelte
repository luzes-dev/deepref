<script lang="ts">
	import EllipsisIcon from '@lucide/svelte/icons/ellipsis';
	import ExternalLinkIcon from '@lucide/svelte/icons/external-link';
	import type { Row } from '@tanstack/table-core';
	import type { ArticleDto } from '$lib/api/generated/models';
	import { Button } from '$lib/components/ui/button';
	import { CopyButton } from '$lib/components/ui/copy-button';
	import * as DropdownMenu from '$lib/components/ui/dropdown-menu';

	let {
		row,
		openArticle
	}: {
		row: Row<ArticleDto>;
		openArticle: (doiKey: string) => void;
	} = $props();

	const article = $derived(row.original);
</script>

<DropdownMenu.Root>
	<DropdownMenu.Trigger>
		{#snippet child({ props })}
			<Button {...props} variant="ghost" size="icon-sm" class="data-[state=open]:bg-muted">
				<EllipsisIcon />
				<span class="sr-only">Open article menu</span>
			</Button>
		{/snippet}
	</DropdownMenu.Trigger>
	<DropdownMenu.Content class="w-40" align="end">
		<DropdownMenu.Group>
			<DropdownMenu.GroupHeading>Actions</DropdownMenu.GroupHeading>
			<DropdownMenu.Item onclick={() => openArticle(article.doi_key)}>
				<ExternalLinkIcon />
				Open article
			</DropdownMenu.Item>
			<DropdownMenu.Item>
				<CopyButton
					text={article.doi}
					variant="ghost"
					size="sm"
					class="h-auto justify-start p-0"
				>
					Copy DOI
				</CopyButton>
			</DropdownMenu.Item>
		</DropdownMenu.Group>
	</DropdownMenu.Content>
</DropdownMenu.Root>
