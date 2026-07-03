<script lang="ts">
	import type { ArticleDto } from '$lib/api/generated/models';
	import { Button } from '$lib/components/ui/button';
	import { cn } from '$lib/utils.js';

	let {
		article,
		selected = false,
		openArticle
	}: {
		article: ArticleDto;
		selected?: boolean;
		openArticle: (doiKey: string) => void;
	} = $props();

	const label = $derived(article.title ?? article.doi);
</script>

<div class="flex max-w-[34rem] flex-col gap-1">
	<Button
		variant="link"
		class={cn(
			'h-auto justify-start p-0 text-left whitespace-normal',
			selected && 'font-semibold'
		)}
		aria-current={selected ? 'true' : undefined}
		onclick={() => openArticle(article.doi_key)}
	>
		<span class="line-clamp-2">{label}</span>
	</Button>
	<div class="truncate text-xs text-muted-foreground">{article.doi}</div>
</div>
