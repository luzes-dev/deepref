<script lang="ts">
	import { resolve } from '$app/paths';
	import { createGetProjectPrisma } from '$lib/api/generated/review/review';
	import {
		createExportProjectArtifact,
		exportProjectArtifact
	} from '$lib/api/generated/exports/exports';
	import * as Alert from '@deepref/ui/alert';
	import * as Dialog from '@deepref/ui/dialog';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import { Skeleton } from '@deepref/ui/skeleton';
	import { PageToolbar, StatePanel } from '@deepref/ui/layout';
	import * as Tabs from '@deepref/ui/tabs';
	import PageTemplate from '$lib/shell/PageTemplate.svelte';
	import { useProjectWorkspaceContext } from '$lib/features/projects/context.svelte.js';
	import { notifyError } from '$lib/features/notifications/toast';
	import DownloadIcon from '@lucide/svelte/icons/download';
	import ImageIcon from '@lucide/svelte/icons/image';
	import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';
	import MoveRightIcon from '@lucide/svelte/icons/move-right';
	import { flowTotals, reconciliationSentence } from '../flow';
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
	let activeView = $state('flow');
	let diagramError = $state(false);
	/** A diagram export waiting for confirmation because records are still unresolved. */
	let pendingExport = $state<{ label: string; run: () => Promise<void> } | undefined>(undefined);
	const projection = $derived(query.data?.data);
	const svgBlob = $derived(svgQuery.data?.data);
	const totals = $derived(projection ? flowTotals(projection) : undefined);
	const awaiting = $derived(projection?.unresolved_records ?? 0);
	const deduplicationHref = $derived(
		resolve('/projects/[projectId]/deduplication', { projectId: workspace.project.id })
	);

	type FlowMetric = readonly [label: string, value: number];
	const flow = $derived<FlowMetric[]>(
		projection
			? [
					['Identified records', projection.identified_records],
					['Linked records', projection.linked_records],
					['Duplicates removed', projection.duplicates_removed],
					[
						'Unresolved records (awaiting duplicate check)',
						projection.unresolved_records
					],
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

	/** Review exports for the Export tab, each with the one-sentence description shown beside it. */
	const exports = [
		{
			kind: 'reports.csv',
			label: 'Reports CSV',
			description:
				'One row per report: title/abstract and full-text decisions, exclusion reason, abstract, authors and study.'
		},
		{
			kind: 'reports.json',
			label: 'Reports JSON',
			description: 'The same report data as typed JSON, for scripts.'
		},
		{
			kind: 'reports.ris',
			label: 'Reports RIS',
			description:
				'Reports for reference managers such as Zotero, EndNote and Rayyan, with abstracts and page ranges.'
		},
		{
			kind: 'reports.bib',
			label: 'Reports BibTeX',
			description:
				'The same reports as BibTeX entries, with LaTeX special characters escaped.'
		},
		{
			kind: 'extraction.csv',
			label: 'Extraction CSV',
			description:
				'One row per study and extraction field: value, status (to verify, confirmed, entered or not extracted), who confirmed it, and the source page and quote.'
		},
		{
			kind: 'appraisal.csv',
			label: 'Appraisal CSV',
			description:
				'One row per appraised report and question: answers, domain and overall judgments, tool name and version, assessor, and evidence.'
		},
		{
			kind: 'included_studies.csv',
			label: 'Included studies CSV',
			description:
				'Included studies with their linked reports, plus included reports not yet grouped into a study.'
		},
		{
			kind: 'prisma.json',
			label: 'PRISMA JSON',
			description: 'The PRISMA flow counts as JSON.'
		},
		{
			kind: 'prisma.svg',
			label: 'PRISMA SVG',
			description: 'The PRISMA flow diagram as an SVG file.'
		},
		{
			kind: 'audit.csv',
			label: 'Audit CSV',
			description:
				'Every screening, study, appraisal and deduplication event, with readable actors and the JSON snapshots last.'
		},
		{
			kind: 'protocol.json',
			label: 'Protocol snapshot',
			description: 'The published protocol as JSON.'
		}
	] as const;

	/**
	 * Inlines the server-rendered diagram so its colours come from the page theme. The markup is
	 * generated by the server from typed counts, and the parser rejects anything that is not SVG.
	 */
	function inlineDiagramAttachment(blob: Blob | undefined) {
		return (element: Element) => {
			let active = true;
			if (blob) {
				void blob
					.text()
					.then((markup) => {
						if (active) replaceWithDiagram(element, markup);
					})
					.catch(() => {
						if (active) diagramError = true;
					});
			}
			return () => {
				active = false;
			};
		};
	}

	function replaceWithDiagram(container: Element, markup: string): void {
		const parsed = new DOMParser().parseFromString(markup, 'image/svg+xml');
		const svg = parsed.documentElement;
		if (svg.localName !== 'svg' || parsed.getElementsByTagName('parsererror').length > 0) {
			throw new TypeError('The PRISMA diagram is not valid SVG');
		}
		svg.setAttribute('class', 'block h-auto w-full min-w-2xl');
		container.replaceChildren(document.importNode(svg, true));
	}

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

	/** Diagram exports ask first while records still await the duplicate check. */
	function requestDiagramExport(label: string, run: () => Promise<void>): void {
		if (awaiting > 0) pendingExport = { label, run };
		else void run();
	}

	async function confirmPendingExport(): Promise<void> {
		const next = pendingExport;
		pendingExport = undefined;
		await next?.run();
	}

	function exportAction(kind: string, label: string): () => void {
		if (kind === 'prisma.svg' || kind === 'prisma.json') {
			return () => requestDiagramExport(label, () => downloadArtifact(kind, kind));
		}
		return () => void downloadArtifact(kind, kind);
	}
</script>

<PageTemplate testId="prisma-page" maxWidth="full" aria-label="PRISMA flow" tabindex="-1">
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
	{:else if projection && totals}
		<Tabs.Root bind:value={activeView} class="min-w-0 gap-4">
			<div class="flex flex-wrap items-center justify-between gap-3">
				<Tabs.List variant="line" aria-label="PRISMA views">
					<Tabs.Trigger value="flow">Review flow</Tabs.Trigger>
					<Tabs.Trigger value="counts">Counts & sources</Tabs.Trigger>
					<Tabs.Trigger value="exports">Export</Tabs.Trigger>
				</Tabs.List>
				<Button
					variant="outline"
					size="sm"
					disabled={Boolean(exporting)}
					onclick={exportAction('prisma.svg', 'PRISMA SVG')}
				>
					<DownloadIcon data-icon="inline-start" aria-hidden="true" />Download SVG
				</Button>
			</div>
			{#if projection.reconciliation_warnings.length > 0}
				<Alert.Root variant="destructive" data-testid="prisma-reconciliation-warning">
					<TriangleAlertIcon aria-hidden="true" />
					<Alert.Title>Some PRISMA counts do not add up</Alert.Title>
					<Alert.Description>
						The flow diagram and its exports may be wrong. Check Counts & sources before
						you use them.
					</Alert.Description>
				</Alert.Root>
			{/if}
			{#if awaiting > 0}
				<Alert.Root variant="warning" data-testid="prisma-awaiting-warning">
					<TriangleAlertIcon aria-hidden="true" />
					<Alert.Title>
						{awaiting === 1
							? '1 record awaits the duplicate check'
							: `${awaiting} records await the duplicate check`}
					</Alert.Title>
					<Alert.Description>
						The diagram shows {awaiting === 1 ? 'it' : 'them'} as a separate box so the flow
						still adds up. Diagram and PRISMA exports stay incomplete until
						{awaiting === 1 ? ' it is' : ' they are'} resolved.
						<a
							class="inline-flex items-center gap-1 font-medium underline underline-offset-2 hover:text-foreground"
							href={deduplicationHref}
						>
							Resolve in Deduplication<MoveRightIcon
								aria-hidden="true"
								class="size-3.5"
							/>
						</a>
					</Alert.Description>
				</Alert.Root>
			{/if}
			<Tabs.Content value="flow" class="min-w-0">
				<section class="min-w-0" aria-labelledby="prisma-diagram-title">
					<div class="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1 pb-1">
						<h2 id="prisma-diagram-title" class="text-sm font-semibold">
							PRISMA 2020 flow diagram
						</h2>
						<p class="text-sm text-muted-foreground tabular-nums">
							{totals.identified} identified · {projection.included_studies}
							{projection.included_studies === 1 ? 'study' : 'studies'} included
						</p>
					</div>
					<p
						class="pb-3 text-xs text-muted-foreground tabular-nums"
						data-testid="prisma-reconciliation"
					>
						{reconciliationSentence(totals)}
					</p>
					<div class="prisma-diagram relative min-w-0 rounded-lg border border-border">
						<!-- Keyboard focus lets users pan the diagram without a pointer. -->
						<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
						<div
							class="overflow-x-auto p-3 sm:p-4"
							tabindex="0"
							role="region"
							aria-label="PRISMA flow diagram, scrollable sideways"
							data-testid="prisma-diagram-scroll"
						>
							{#if svgQuery.error}
								<p class="text-sm text-destructive" role="alert">
									Unable to load the canonical diagram: {svgQuery.error.message}
								</p>
							{:else if svgBlob && !diagramError}
								<div {@attach inlineDiagramAttachment(svgBlob)}></div>
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
						<div
							class="pointer-events-none absolute inset-y-0 right-0 w-10 rounded-r-lg bg-linear-to-l from-background to-transparent md:hidden"
							aria-hidden="true"
						></div>
					</div>
					<p
						class="mt-2 flex items-center gap-1.5 text-xs text-muted-foreground md:hidden"
					>
						<MoveRightIcon aria-hidden="true" class="size-3.5" />
						Swipe sideways for the excluded and removed branches. Exact values are also in
						Counts &amp; sources.
					</p>
				</section>
			</Tabs.Content>
			<Tabs.Content value="exports" class="min-w-0">
				<section class="min-w-0" aria-labelledby="export-title">
					<div class="border-b border-border/70 pb-3">
						<h2 id="export-title" class="text-lg font-semibold">Export evidence</h2>
						<p class="mt-1 text-sm text-muted-foreground">
							Download the diagram or review data for your report.
						</p>
					</div>
					<div class="flex flex-wrap gap-x-4 gap-y-5 pt-4">
						{#each exports as item (item.kind)}
							<div class="flex w-full min-w-0 flex-col items-start gap-1.5 sm:w-72">
								<Button
									variant="outline"
									disabled={Boolean(exporting)}
									onclick={exportAction(item.kind, item.label)}
								>
									<DownloadIcon data-icon="inline-start" aria-hidden={true} />
									{item.label}
								</Button>
								<p class="text-xs text-muted-foreground">{item.description}</p>
							</div>
						{/each}
						<div class="flex w-full min-w-0 flex-col items-start gap-1.5 sm:w-72">
							<Button
								disabled={Boolean(exporting)}
								onclick={() =>
									requestDiagramExport('PRISMA PNG', () => downloadPng())}
							>
								<ImageIcon data-icon="inline-start" aria-hidden={true} />
								PRISMA PNG
							</Button>
							<p class="text-xs text-muted-foreground">
								The PRISMA flow diagram as a PNG image.
							</p>
						</div>
					</div>
				</section>
			</Tabs.Content>
			<Tabs.Content value="counts" class="min-w-0">
				<PageToolbar label="PRISMA projection status">
					<div class="flex flex-wrap items-center gap-2 text-sm">
						<Badge variant="secondary"
							>{projection.as_of
								? `Updated ${new Date(projection.as_of).toLocaleDateString()}`
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
				<div class="mb-3 flex items-baseline justify-between gap-3">
					<h2
						id="prisma-counts-title"
						class="text-sm font-semibold tracking-snug-caps text-muted-foreground uppercase"
					>
						Audit-ready flow counts
					</h2>
					<span class="text-xs text-muted-foreground">Server projection</span>
				</div>
				<dl class="grid gap-x-8 sm:grid-cols-2 lg:grid-cols-3">
					{#each flow as [label, value] (label)}
						<div
							class="flex items-baseline justify-between gap-4 border-b py-3 text-sm"
						>
							<dt class="text-muted-foreground">{label}</dt>
							<dd class="font-semibold tabular-nums">{value}</dd>
						</div>
					{/each}
					{#if groupedReports !== undefined}
						<div
							class="flex items-baseline justify-between gap-4 border-b py-3 text-sm"
						>
							<dt class="text-muted-foreground">Grouped reports</dt>
							<dd class="font-semibold tabular-nums">{groupedReports}</dd>
						</div>
					{/if}
				</dl>
				<section class="pt-2" aria-labelledby="exclusion-reasons-title">
					<h2 id="exclusion-reasons-title" class="text-lg font-semibold">
						Full-text exclusion reasons
					</h2>
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
										<span class="text-muted-foreground">({reason.code})</span
										></span
									>
									<span class="font-medium tabular-nums">{reason.count}</span>
								</li>
							{/each}
						</ul>
					{/if}
				</section>
			</Tabs.Content>
		</Tabs.Root>
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

<Dialog.Root
	open={pendingExport !== undefined}
	onOpenChange={(open) => {
		if (!open) pendingExport = undefined;
	}}
>
	<Dialog.Content>
		<Dialog.Header>
			<Dialog.Title>Export an incomplete PRISMA figure?</Dialog.Title>
			<Dialog.Description>
				{awaiting === 1 ? '1 record is' : `${awaiting} records are`} still waiting for the duplicate
				check. The {pendingExport?.label ?? 'export'} shows
				{awaiting === 1 ? 'it' : 'them'} as awaiting, so it is not a final review figure.
			</Dialog.Description>
		</Dialog.Header>
		<Dialog.Footer>
			<Button variant="outline" onclick={() => (pendingExport = undefined)}>Cancel</Button>
			<a
				class="inline-flex h-8 items-center rounded-md px-3 text-sm font-medium underline underline-offset-2 hover:text-foreground"
				href={deduplicationHref}>Resolve first</a
			>
			<Button onclick={() => void confirmPendingExport()}>Export anyway</Button>
		</Dialog.Footer>
	</Dialog.Content>
</Dialog.Root>

<style>
	/*
	 * The on-page diagram is inlined, so these custom properties feed its SVG styles from the
	 * active theme. The downloaded SVG and PNG do not get them and keep their light fallbacks,
	 * so a publication figure stays white and prints the same in any viewer.
	 */
	.prisma-diagram {
		--prisma-canvas: var(--background);
		--prisma-box: var(--card);
		--prisma-edge: var(--muted-foreground);
		--prisma-ink: var(--foreground);
		--prisma-muted: var(--muted-foreground);
		--prisma-arrow: var(--muted-foreground);
		--prisma-awaiting: var(--warning-surface);
		--prisma-awaiting-edge: var(--warning);
		--prisma-font: var(--font-sans);
	}
</style>
