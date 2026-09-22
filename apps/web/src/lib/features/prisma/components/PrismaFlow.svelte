<script lang="ts">
	import { createGetProjectPrisma } from '$lib/api/generated/review/review';
	import {
		createExportProjectArtifact,
		exportProjectArtifact
	} from '$lib/api/generated/exports/exports';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import { Skeleton } from '@deepref/ui/skeleton';
	import { PageToolbar, StatePanel, MetricTile } from '@deepref/ui/layout';
	import PageTemplate from '$lib/shell/PageTemplate.svelte';
	import { useProjectWorkspaceContext } from '$lib/features/projects/context.svelte.js';
	import { notifyError } from '$lib/features/notifications/toast';
	import DownloadIcon from '@lucide/svelte/icons/download';
	import ImageIcon from '@lucide/svelte/icons/image';
	import {
		attachmentFilename,
		downloadBlob,
		downloadPrismaPng,
		pngAttachmentFilename
	} from '../png';

	const workspace = useProjectWorkspaceContext();
	const query = createGetProjectPrisma(
		() => workspace.project.id,
		() => ({ query: { enabled: Boolean(workspace.project.id), staleTime: 0 } })
	);
	const svgQuery = createExportProjectArtifact(
		() => workspace.project.id,
		() => 'prisma.svg',
		() => ({ query: { enabled: Boolean(workspace.project.id), staleTime: 0 } })
	);
	let exporting = $state<string | undefined>(undefined);
	let diagramError = $state(false);
	const projection = $derived(query.data?.data);
	const svgBlob = $derived(svgQuery.data?.data);

	const exports = [
		['reports.csv', 'Reports CSV'],
		['reports.json', 'Reports JSON'],
		['reports.ris', 'Reports RIS'],
		['reports.bib', 'Reports BibTeX'],
		['prisma.json', 'PRISMA JSON'],
		['prisma.svg', 'PRISMA SVG'],
		['audit.csv', 'Audit CSV'],
		['protocol.json', 'Protocol snapshot']
	] as const;

	function canonicalSvgAttachment(blob: Blob | undefined) {
		return (element: Element) => {
			if (!(element instanceof HTMLImageElement)) {
				throw new TypeError('The PRISMA diagram requires an image element');
			}
			const url = blob ? URL.createObjectURL(blob) : undefined;
			element.src = url ?? '';
			return () => {
				if (url) URL.revokeObjectURL(url);
			};
		};
	}

	type FlowMetric = readonly [label: string, value: number];
	const flow = $derived<FlowMetric[]>(
		projection
			? [
					['Identified records', projection.identified_records],
					['Linked records', projection.linked_records],
					['Duplicates removed', projection.duplicates_removed],
					['Unresolved records', projection.unresolved_records],
					['Source-canonical reports', projection.source_canonical_reports],
					['Manually created reports', projection.manually_created_reports],
					['Screened records', projection.screened_records],
					['Title/abstract excluded', projection.title_abstract_excluded],
					['Title/abstract pending', projection.title_abstract_pending],
					['Reports sought', projection.reports_sought],
					['Reports not retrieved', projection.reports_not_retrieved],
					['Full texts assessed', projection.full_text_assessed],
					['Full-text pending', projection.full_text_pending],
					['Full-text excluded', projection.full_text_excluded],
					['Full-text included', projection.full_text_included],
					['Included reports not grouped', projection.included_reports_not_grouped],
					['Included studies', projection.included_studies]
				]
			: []
	);
	const groupedReports = $derived.by(() => {
		if (!projection) return undefined;
		const grouped = projection.full_text_included - projection.included_reports_not_grouped;
		return grouped >= 0 ? grouped : undefined;
	});

	async function downloadArtifact(kind: string, fallback: string): Promise<void> {
		exporting = kind;
		try {
			const response = await exportProjectArtifact(workspace.project.id, kind);
			downloadBlob(response.data, attachmentFilename(response.headers, fallback));
		} catch (error) {
			notifyError('Export unavailable', error);
		} finally {
			exporting = undefined;
		}
	}

	async function downloadPng(): Promise<void> {
		exporting = 'prisma.png';
		try {
			const response = await exportProjectArtifact(workspace.project.id, 'prisma.svg');
			await downloadPrismaPng(
				response.data,
				pngAttachmentFilename(
					response.headers,
					`deepref-${workspace.project.id}-prisma.png`
				)
			);
		} catch (error) {
			notifyError('Export unavailable', error);
		} finally {
			exporting = undefined;
		}
	}
</script>

<PageTemplate testId="prisma-page" maxWidth="default" aria-label="PRISMA flow" tabindex="-1">
	{#if query.isPending}
		<div class="p-4 sm:p-6">
			<StatePanel
				state="loading"
				title="Assembling PRISMA projection"
				description="Reconciling screening, retrieval, and inclusion counts."
			/>
		</div>
	{:else if query.error}
		<div class="p-4 sm:p-6">
			<StatePanel
				state="error"
				title="PRISMA projection unavailable"
				description={`Unable to load the PRISMA projection: ${query.error.message}`}
			/>
		</div>
	{:else if projection}
		<PageToolbar label="PRISMA projection status">
			<div class="flex flex-wrap items-center gap-2 text-sm">
				<Badge variant="secondary"
					>{projection.as_of
						? `Updated ${new Date(projection.as_of).toLocaleDateString()}`
						: 'No decisions yet'}</Badge
				>
				<Badge variant="outline">{projection.pending_dedupe_proposals} pending dedupe</Badge
				>
				<Badge variant="outline"
					>Max per-report screening revision: {projection.screening_high_watermark}</Badge
				>
			</div>
		</PageToolbar>

		<div class="grid gap-6 lg:grid-cols-[minmax(0,1.35fr)_minmax(20rem,0.65fr)]">
			<section class="min-w-0" aria-labelledby="prisma-diagram-title">
				<div
					class="flex flex-wrap items-start justify-between gap-3 border-b border-border/70 pb-3"
				>
					<div>
						<h2 id="prisma-diagram-title" class="text-lg font-semibold">Review flow</h2>
						<p class="mt-1 text-sm text-muted-foreground">
							From the first search to the final included studies.
						</p>
					</div>
					{#if svgQuery.isFetching}<Badge variant="outline">Loading diagram…</Badge>{/if}
				</div>
				<div class="pt-4">
					{#if svgQuery.error}
						<p class="text-sm text-destructive" role="alert">
							Unable to load the canonical diagram: {svgQuery.error.message}
						</p>
					{:else if svgBlob && !diagramError}
						<img
							{@attach canonicalSvgAttachment(svgBlob)}
							alt="PRISMA flow diagram showing identification, screening, retrieval, assessment, and inclusion counts"
							class="h-auto w-full bg-white"
							onerror={() => (diagramError = true)}
						/>
					{:else if diagramError}
						<p class="text-sm text-destructive" role="alert">
							The canonical diagram could not be rendered.
						</p>
					{:else}
						<Skeleton
							class="h-96 w-full"
							aria-label="Loading canonical PRISMA diagram"
						/>
					{/if}
				</div>
			</section>

			<section class="min-w-0" aria-labelledby="export-title">
				<div class="border-b border-border/70 pb-3">
					<h2 id="export-title" class="text-lg font-semibold">Export evidence</h2>
					<p class="mt-1 text-sm text-muted-foreground">
						Download the diagram or review data for your report.
					</p>
				</div>
				<div class="flex flex-wrap gap-2 pt-4">
					{#each exports as [kind, label] (kind)}
						<Button
							variant="outline"
							disabled={Boolean(exporting)}
							onclick={() => void downloadArtifact(kind, kind)}
						>
							<DownloadIcon data-icon="inline-start" aria-hidden={true} />
							{label}
						</Button>
					{/each}
					<Button disabled={Boolean(exporting)} onclick={() => void downloadPng()}>
						<ImageIcon data-icon="inline-start" aria-hidden={true} />
						PRISMA PNG
					</Button>
				</div>
			</section>
		</div>

		<details class="disclosure">
			<summary>All review counts</summary>
			<div class="mb-3 flex items-baseline justify-between gap-3">
				<h2
					id="prisma-counts-title"
					class="text-sm font-semibold tracking-[0.08em] text-muted-foreground uppercase"
				>
					Audit-ready flow counts
				</h2>
				<span class="text-xs text-muted-foreground">Server projection</span>
			</div>
			<div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
				{#each flow as [label, value] (label)}
					<MetricTile {label} {value} class="[font-variant-numeric:tabular-nums]" />
				{/each}
				{#if groupedReports !== undefined}
					<MetricTile
						label="Grouped reports"
						value={groupedReports}
						class="[font-variant-numeric:tabular-nums]"
					/>
				{/if}
			</div>
		</details>
		<section class="pt-2" aria-labelledby="exclusion-reasons-title">
			<h2 id="exclusion-reasons-title" class="text-lg font-semibold">
				Full-text exclusion reasons
			</h2>
			{#if projection.full_text_exclusions.length === 0}
				<p class="mt-2 text-sm text-muted-foreground">No full-text exclusions recorded.</p>
			{:else}
				<ul class="mt-3 grid gap-2 sm:grid-cols-2" aria-label="Full-text exclusion reasons">
					{#each projection.full_text_exclusions as reason (reason.id)}
						<li
							class="flex justify-between gap-4 border-b border-border/60 py-2 text-sm last:border-b-0"
						>
							<span
								>{reason.label}
								<span class="text-muted-foreground">({reason.code})</span></span
							>
							<span class="font-medium tabular-nums">{reason.count}</span>
						</li>
					{/each}
				</ul>
			{/if}
		</section>
	{:else}
		<div class="p-4 sm:p-6">
			<StatePanel
				state="empty"
				title="No PRISMA projection"
				description="A projection will appear after this project has evidence activity."
			/>
		</div>
	{/if}
</PageTemplate>
