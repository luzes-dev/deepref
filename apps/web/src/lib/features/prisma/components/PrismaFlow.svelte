<script lang="ts">
	import { createGetProjectPrisma } from '$lib/api/generated/review/review';
	import {
		createExportProjectArtifact,
		exportProjectArtifact
	} from '$lib/api/generated/exports/exports';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import { Skeleton } from '$lib/components/ui/skeleton';
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
			const node = element as HTMLImageElement;
			const url = blob ? URL.createObjectURL(blob) : undefined;
			node.src = url ?? '';
			return () => {
				if (url) URL.revokeObjectURL(url);
			};
		};
	}

	const flow = $derived(
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

<section class="flex h-full flex-col gap-4 overflow-auto p-4" aria-labelledby="prisma-title">
	<div class="flex flex-wrap items-start justify-between gap-3">
		<div>
			<h1 id="prisma-title" class="text-2xl font-semibold tracking-tight">PRISMA flow</h1>
			<p class="text-sm text-muted-foreground">
				Deterministic screening, retrieval, and inclusion reconciliation for this project.
			</p>
		</div>
	</div>

	{#if exportError}
		<div
			class="rounded-md border border-destructive/40 p-3 text-sm text-destructive"
			role="alert"
		>
			{exportError}
		</div>
	{/if}

	{#if query.isPending}
		<div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
			{#each [0, 1, 2, 3, 4, 5] as placeholder (placeholder)}
				<Skeleton class="h-20" />
			{/each}
		</div>
	{:else if query.error}
		<div
			class="rounded-md border border-destructive/40 p-4 text-sm text-destructive"
			role="alert"
		>
			Unable to load the PRISMA projection: {query.error.message}
		</div>
	{:else if projection}
		<div class="flex flex-wrap items-center gap-2 text-sm">
			<Badge variant="secondary"
				>{projection.as_of ? `As of ${projection.as_of}` : 'No decisions yet'}</Badge
			>
			<Badge variant="outline">{projection.pending_dedupe_proposals} pending dedupe</Badge>
			<Badge variant="outline"
				>Max per-report screening revision: {projection.screening_high_watermark}</Badge
			>
		</div>
		<div class="grid gap-3 sm:grid-cols-2 lg:grid-cols-3">
			{#each flow as [label, value] (label)}
				<div class="rounded-lg border bg-card p-4 shadow-xs">
					<div class="text-sm text-muted-foreground">{label}</div>
					<div class="mt-1 text-2xl font-semibold tabular-nums">{value}</div>
				</div>
			{/each}
			{#if groupedReports !== undefined}
				<div class="rounded-lg border bg-card p-4 shadow-xs">
					<div class="text-sm text-muted-foreground">Grouped reports</div>
					<div class="mt-1 text-2xl font-semibold tabular-nums">{groupedReports}</div>
				</div>
			{/if}
		</div>
		<div class="rounded-lg border bg-card p-4 shadow-xs" aria-labelledby="prisma-diagram-title">
			<div class="mb-3 flex items-center justify-between gap-2">
				<div>
					<h2 id="prisma-diagram-title" class="font-medium">Canonical PRISMA diagram</h2>
					<p class="text-sm text-muted-foreground">
						Rendered from the same server projection used by these counts.
					</p>
				</div>
				{#if svgQuery.isFetching}<span class="text-sm text-muted-foreground"
						>Loading diagram…</span
					>{/if}
			</div>
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
				<Skeleton class="h-96 w-full" aria-label="Loading canonical PRISMA diagram" />
			{/if}
		</div>
		<div class="rounded-lg border bg-card p-4 shadow-xs" aria-labelledby="export-title">
			<div class="mb-3">
				<h2 id="export-title" class="font-medium">Export evidence</h2>
				<p class="text-sm text-muted-foreground">
					Download complete, bounded artifacts with deterministic filenames.
				</p>
			</div>
			<div class="flex flex-wrap gap-2">
				{#each exports as [kind, label] (kind)}
					<Button
						variant="outline"
						disabled={Boolean(exporting)}
						onclick={() => void downloadArtifact(kind, kind)}
					>
						<DownloadIcon class="mr-2 size-4" aria-hidden={true} />
						{label}
					</Button>
				{/each}
				<Button disabled={Boolean(exporting)} onclick={() => void downloadPng()}>
					<ImageIcon class="mr-2 size-4" aria-hidden={true} />
					PRISMA PNG
				</Button>
			</div>
		</div>
		<div class="rounded-lg border bg-muted/20 p-4">
			<h2 class="font-medium">Full-text exclusion reasons</h2>
			{#if projection.full_text_exclusions.length === 0}
				<p class="mt-2 text-sm text-muted-foreground">No full-text exclusions recorded.</p>
			{:else}
				<ul class="mt-2 grid gap-2 sm:grid-cols-2" aria-label="Full-text exclusion reasons">
					{#each projection.full_text_exclusions as reason (reason.id)}
						<li class="flex justify-between gap-4 text-sm">
							<span
								>{reason.label}
								<span class="text-muted-foreground">({reason.code})</span></span
							>
							<span class="font-medium tabular-nums">{reason.count}</span>
						</li>
					{/each}
				</ul>
			{/if}
		</div>
	{:else}
		<p class="text-sm text-muted-foreground">No PRISMA projection is available.</p>
	{/if}
</section>
