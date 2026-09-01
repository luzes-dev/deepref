<script lang="ts">
	import { createGetProjectPrisma } from '$lib/api/generated/review/review';
	import {
		createExportProjectArtifact,
		exportProjectArtifact
	} from '$lib/api/generated/exports/exports';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import {
		PageHeader,
		PageToolbar,
		StatePanel,
		Surface,
		MetricTile
	} from '$lib/components/layout';
	import * as Alert from '$lib/components/ui/alert';
	import { useProjectWorkspaceContext } from '$lib/components/project/context.svelte.js';
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
	let exportError = $state<string | undefined>(undefined);
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

	function errorMessage(error: unknown): string {
		return error instanceof Error ? error.message : 'The export could not be downloaded.';
	}

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
		exportError = undefined;
		try {
			const response = await exportProjectArtifact(workspace.project.id, kind);
			downloadBlob(response.data, attachmentFilename(response.headers, fallback));
		} catch (error) {
			exportError = errorMessage(error);
		} finally {
			exporting = undefined;
		}
	}

	async function downloadPng(): Promise<void> {
		exporting = 'prisma.png';
		exportError = undefined;
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
			exportError = errorMessage(error);
		} finally {
			exporting = undefined;
		}
	}
</script>

<section
	class="flex h-full min-h-0 flex-col overflow-auto bg-background"
	aria-label="PRISMA flow"
	tabindex="-1"
	data-testid="prisma-page"
>
	<div class="mx-auto flex w-full max-w-[1440px] flex-col gap-5 p-4 sm:gap-6 sm:p-6 lg:p-8">
		<PageHeader
			eyebrow="Evidence workspace / Analysis"
			title="PRISMA flow"
			description="Deterministic screening, retrieval, and inclusion reconciliation for this project."
			class="[&>div>h1]:!font-serif"
		/>

		{#if exportError}
			<Alert.Root variant="destructive" data-testid="prisma-export-error">
				<Alert.Title>Export unavailable</Alert.Title>
				<Alert.Description>{exportError}</Alert.Description>
			</Alert.Root>
		{/if}

		{#if query.isPending}
			<Surface as="section" tone="subtle" class="p-4 sm:p-6">
				<StatePanel
					state="loading"
					title="Assembling PRISMA projection"
					description="Reconciling screening, retrieval, and inclusion counts."
				/>
			</Surface>
		{:else if query.error}
			<Surface as="section" tone="subtle" class="p-4 sm:p-6">
				<StatePanel
					state="error"
					title="PRISMA projection unavailable"
					description={`Unable to load the PRISMA projection: ${query.error.message}`}
				/>
			</Surface>
		{:else if projection}
			<PageToolbar label="PRISMA projection status">
				<div class="flex flex-wrap items-center gap-2 text-sm">
					<Badge variant="secondary"
						>{projection.as_of
							? `As of ${projection.as_of}`
							: 'No decisions yet'}</Badge
					>
					<Badge variant="outline"
						>{projection.pending_dedupe_proposals} pending dedupe</Badge
					>
					<Badge variant="outline"
						>Max per-report screening revision: {projection.screening_high_watermark}</Badge
					>
				</div>
			</PageToolbar>

			<section aria-labelledby="prisma-counts-title">
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
			</section>

			<div class="grid gap-5 lg:grid-cols-[minmax(0,1.35fr)_minmax(20rem,0.65fr)]">
				<Surface
					as="section"
					tone="default"
					class="min-w-0 overflow-hidden"
					label="PRISMA diagram"
				>
					<div
						class="flex flex-wrap items-start justify-between gap-3 border-b border-border/70 p-4 sm:p-5"
					>
						<div>
							<h2 id="prisma-diagram-title" class="text-lg font-semibold">
								Canonical PRISMA diagram
							</h2>
							<p class="mt-1 text-sm text-muted-foreground">
								Rendered from the same server projection used by these counts.
							</p>
						</div>
						{#if svgQuery.isFetching}<Badge variant="outline">Loading diagram…</Badge
							>{/if}
					</div>
					<div class="p-4 sm:p-5">
						{#if svgQuery.error}
							<p class="text-sm text-destructive" role="alert">
								Unable to load the canonical diagram: {svgQuery.error.message}
							</p>
						{:else if svgBlob && !diagramError}
							<img
								{@attach canonicalSvgAttachment(svgBlob)}
								alt="PRISMA flow diagram showing identification, screening, retrieval, assessment, and inclusion counts"
								class="h-auto w-full"
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
				</Surface>

				<Surface as="section" tone="subtle" class="min-w-0" label="Evidence exports">
					<div class="border-b border-border/70 p-4 sm:p-5">
						<h2 id="export-title" class="text-lg font-semibold">Export evidence</h2>
						<p class="mt-1 text-sm text-muted-foreground">
							Download complete, bounded artifacts with deterministic filenames.
						</p>
					</div>
					<div class="flex flex-wrap gap-2 p-4 sm:p-5">
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
				</Surface>
			</div>

			<Surface
				as="section"
				tone="subtle"
				class="p-4 sm:p-5"
				label="Full-text exclusion reasons"
			>
				<h2 class="text-lg font-semibold">Full-text exclusion reasons</h2>
				{#if projection.full_text_exclusions.length === 0}
					<p class="mt-2 text-sm text-muted-foreground">
						No full-text exclusions recorded.
					</p>
				{:else}
					<ul
						class="mt-3 grid gap-2 sm:grid-cols-2"
						aria-label="Full-text exclusion reasons"
					>
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
			</Surface>
		{:else}
			<Surface as="section" tone="subtle" class="p-4 sm:p-6">
				<StatePanel
					state="empty"
					title="No PRISMA projection"
					description="A projection will appear after this project has evidence activity."
				/>
			</Surface>
		{/if}
	</div>
</section>
