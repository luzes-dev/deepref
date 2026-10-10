<script lang="ts">
	import * as Alert from '@deepref/ui/alert';
	import { Button } from '@deepref/ui/button';
	import { CopyButton } from '@deepref/ui/copy-button';
	import { Skeleton } from '@deepref/ui/skeleton';
	import { createGetProjectReport } from '#lib/api/generated/reports/reports.js';
	import { StatePanel } from '@deepref/ui/layout';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import PanelRightCloseIcon from '@lucide/svelte/icons/panel-right-close';
	import PanelRightOpenIcon from '@lucide/svelte/icons/panel-right-open';
	import XIcon from '@lucide/svelte/icons/x';
	import { useProjectWorkspaceContext } from '../context.svelte.js';
	import { reportLabel } from '../report-label';

	let {
		collapsed = false,
		onToggleCollapse = () => {}
	}: {
		collapsed?: boolean;
		onToggleCollapse?: () => void;
	} = $props();

	const workspace = useProjectWorkspaceContext();

	const articleQuery = createGetProjectReport(
		() => workspace.project.id,
		() => workspace.selectedArticle ?? '',
		() => ({
			query: {
				enabled: Boolean(workspace.project.id && workspace.selectedArticle),
				staleTime: 0
			}
		})
	);
	const article = $derived(articleQuery.data?.data);
</script>

<aside
	class="flex h-full min-h-0 flex-col border-l bg-background"
	data-testid="article-inspector"
	data-selected={workspace.selectedArticle ? 'true' : 'false'}
>
	{#if collapsed}
		<div class="flex h-full flex-col items-center gap-3 border-b px-2 py-4">
			<Button
				variant="ghost"
				size="icon"
				onclick={onToggleCollapse}
				aria-label="Expand article inspector"
			>
				<PanelRightOpenIcon data-icon />
			</Button>
			<span class="text-2xs text-muted-foreground">Details</span>
		</div>
	{:else}
		<div class="flex items-center justify-between gap-2 border-b p-4">
			<div class="min-w-0">
				<h2 class="truncate text-sm font-medium">Article details</h2>
			</div>
			<div class="flex items-center gap-1">
				<Button
					variant="ghost"
					size="icon"
					class="hidden md:inline-flex"
					onclick={onToggleCollapse}
					aria-label="Collapse article inspector"
				>
					<PanelRightCloseIcon data-icon />
				</Button>
				{#if workspace.selectedArticle}
					<Button
						variant="ghost"
						size="icon"
						class="hidden md:inline-flex"
						onclick={workspace.clearArticle}
						aria-label="Deselect article"
					>
						<XIcon data-icon />
					</Button>
				{/if}
			</div>
		</div>

		<!-- Keyboard users need to focus this region to scroll long details. -->
		<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
		<div
			class="min-h-0 flex-1 overflow-auto p-4"
			tabindex="0"
			role="region"
			aria-label="Article details"
		>
			{#if !workspace.selectedArticle}
				<StatePanel
					state="empty"
					title="No article selected"
					description="Select an article to inspect its metadata and provenance."
				/>
			{:else if articleQuery.error}
				<Alert.Root variant="destructive">
					<CircleAlertIcon />
					<Alert.Title>Article unavailable</Alert.Title>
					<Alert.Description>{articleQuery.error.message}</Alert.Description>
					<Alert.Action class="flex gap-1">
						<Button
							variant="ghost"
							size="sm"
							onclick={() => void articleQuery.refetch()}>Try again</Button
						>
						<Button variant="ghost" size="sm" onclick={workspace.clearArticle}
							>Clear selection</Button
						>
					</Alert.Action>
				</Alert.Root>
			{:else if articleQuery.isPending}
				<div class="flex flex-col gap-3">
					<Skeleton class="h-8 w-4/5" />
					<Skeleton class="h-4 w-2/3" />
					<Skeleton class="h-32" />
				</div>
			{:else if article}
				<div class="flex flex-col gap-4">
					<div class="text-xs text-muted-foreground">
						{article.issued_year ?? 'Year unavailable'} · {article.type ??
							'Type unavailable'}
						{#if article.publisher}<span class="mt-1 block">{article.publisher}</span
							>{/if}
					</div>
					<div class="flex items-start justify-between gap-3">
						<div class="min-w-0">
							<h3 class="text-lg font-semibold wrap-break-word">
								{reportLabel(article)}
							</h3>
							{#if article.doi}
								<p class="text-sm break-all text-muted-foreground">{article.doi}</p>
							{/if}
						</div>
						{#if article.doi}<CopyButton text={article.doi} />{/if}
					</div>

					<dl class="flex flex-wrap gap-x-6 gap-y-2 text-xs">
						<div>
							<dt class="text-muted-foreground">Citations</dt>
							<dd class="mt-1 text-base font-semibold tabular-nums">
								{article.total_citations}
							</dd>
						</div>
						<div>
							<dt class="text-muted-foreground">References</dt>
							<dd class="mt-1 text-base font-semibold tabular-nums">
								{article.references_count}
							</dd>
						</div>
					</dl>
					{#if article.metrics_stale}
						<p
							class="text-xs text-muted-foreground"
							data-testid="article-stale-metrics"
						>
							Metrics are stale · Last computed {article.metrics_as_of
								? new Date(article.metrics_as_of).toLocaleString()
								: 'not yet'}.
						</p>
					{:else if article.metrics_as_of}
						<p class="text-xs text-muted-foreground">
							Metrics as of {new Date(article.metrics_as_of).toLocaleString()}
						</p>
					{/if}
					<section aria-label="Abstract" class="flex flex-col gap-2">
						<h4 class="text-sm font-semibold">Abstract</h4>
						<p class="text-sm leading-relaxed wrap-break-word">
							{article.abstract ?? 'No abstract available.'}
						</p>
					</section>
					<details>
						<summary class="cursor-pointer text-xs text-muted-foreground"
							>Source metadata</summary
						>
						<pre
							class="mt-3 max-h-80 overflow-auto bg-muted p-3 text-xs">{JSON.stringify(
								article.raw,
								null,
								2
							)}</pre>
					</details>
				</div>
			{:else}
				<StatePanel
					state="error"
					title="Article details unavailable"
					description="The selected article did not return a usable record. Try again or clear the selection."
				/>
			{/if}
		</div>
	{/if}
</aside>
