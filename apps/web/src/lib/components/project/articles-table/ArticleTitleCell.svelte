<script lang="ts">
	import type { ReportDto } from '$lib/api/generated/models';
	import { Button } from '$lib/components/ui/button';
	import { cn } from '$lib/utils.js';
	import { reportLabel } from '../report-label';

	let {
		article,
		selected = false,
		openArticle
	}: {
		article: ReportDto;
		selected?: boolean;
		openArticle: (reportId: string) => void;
	} = $props();

	const label = $derived(reportLabel(article));
</script>

<div class="flex max-w-[34rem] flex-col gap-1">
	<Button
		variant="link"
		class={cn(
			'h-auto justify-start p-0 text-left whitespace-normal',
			selected && 'font-semibold'
		)}
		aria-current={selected ? 'true' : undefined}
		onclick={() => openArticle(article.report_id)}
	>
		<span class="line-clamp-2">{label}</span>
	</Button>
	{#if article.doi}
		<div class="truncate text-xs text-muted-foreground">{article.doi}</div>
	{/if}
</div>
