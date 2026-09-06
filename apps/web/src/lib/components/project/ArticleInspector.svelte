<script lang="ts">
	import * as Alert from '$lib/components/ui/alert';
	import { Button } from '$lib/components/ui/button';
	import { CopyButton } from '$lib/components/ui/copy-button';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { createGetProjectReport } from '$lib/api/generated/reports/reports';
	import { MetricTile, StatePanel, Surface } from '$lib/components/layout';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import PanelRightCloseIcon from '@lucide/svelte/icons/panel-right-close';
	import PanelRightOpenIcon from '@lucide/svelte/icons/panel-right-open';
	import XIcon from '@lucide/svelte/icons/x';
	import { useProjectWorkspaceContext } from './context.svelte.js';
	import { reportLabel } from './report-label';

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
			<div class="flex min-h-0 flex-1 items-center justify-center">
				<div
					class="flex -rotate-180 items-center gap-3 text-muted-foreground [writing-mode:vertical-rl]"
				>
					<span class="text-xs font-medium tracking-[0.2em] uppercase">Inspector</span>
					<span class="max-h-48 overflow-hidden text-sm font-medium text-ellipsis">
						{workspace.selectedArticle ? 'Article' : 'No article'}
					</span>
				</div>
			</div>
		</div>
	{:else}
		<div class="flex items-center justify-between gap-2 border-b p-4">
			<div class="min-w-0">
				<h2 class="truncate font-medium">Article inspector</h2>
				<p class="truncate text-xs text-muted-foreground">
					{workspace.selectedArticle ?? 'No article selected'}
				</p>
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

		<div class="min-h-0 flex-1 overflow-auto p-4">
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
					{#if article.metrics_stale}
						<Alert.Root data-testid="article-stale-metrics">
							<CircleAlertIcon />
							<Alert.Title>Metrics are stale</Alert.Title>
							<Alert.Description>
								Last computed {article.metrics_as_of
									? new Date(article.metrics_as_of).toLocaleString()
									: 'not yet'}.
							</Alert.Description>
						</Alert.Root>
					{:else if article.metrics_as_of}
						<p class="text-sm text-muted-foreground">
							Metrics as of {new Date(article.metrics_as_of).toLocaleString()}
						</p>
					{/if}
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

					<div class="grid grid-cols-2 gap-3">
						<MetricTile
							label="Total citations"
							value={article.total_citations}
							tone="warning"
							class="[font-variant-numeric:tabular-nums]"
						/>
						<MetricTile
							label="References"
							value={article.references_count}
							tone="info"
							class="[font-variant-numeric:tabular-nums]"
						/>
						<MetricTile
							label="Year"
							value={article.issued_year ?? '-'}
							tone="default"
							class="[font-variant-numeric:tabular-nums]"
						/>
						<MetricTile label="Type" value={article.type ?? 'unknown'} tone="default" />
					</div>

					<Surface as="section" tone="inset" class="flex flex-col gap-3 p-4">
						<div>
							<h4 class="font-medium">Metadata</h4>
							<p class="mt-1 text-sm text-muted-foreground">{article.publisher}</p>
						</div>
						<p class="text-sm leading-relaxed wrap-break-word">
							{article.abstract ?? 'No abstract available.'}
						</p>
						<pre
							class="max-h-80 overflow-auto rounded-md bg-muted p-3 text-xs [font-variant-numeric:tabular-nums]">{JSON.stringify(
								article.raw,
								null,
								2
							)}</pre>
					</Surface>
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
