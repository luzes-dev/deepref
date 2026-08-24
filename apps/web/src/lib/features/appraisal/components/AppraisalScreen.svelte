<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import { page } from '$app/state';
	import {
		createCompleteReportAppraisal,
		createListAppraisalDefinitions,
		createListReportAppraisals,
		getListReportAppraisalsQueryKey
	} from '$lib/api/generated/appraisal/appraisal';
	import {
		createListDocumentBlocks,
		createListReportDocuments
	} from '$lib/api/generated/documents/documents';
	import { createListProjectReports } from '$lib/api/generated/reports/reports';
	import type { CompleteAppraisalRequest } from '$lib/api/generated/models';
	import { useQueryClient } from '@tanstack/svelte-query';
	import * as Card from '$lib/components/ui/card';
	import { parseAppraisalLocation, updateAppraisalLocation } from '../url';
	import AppraisalForm from './AppraisalForm.svelte';

	let { projectId }: { projectId: string } = $props();
	const queryClient = useQueryClient();
	let error = $state<string | undefined>();

	const location = $derived(parseAppraisalLocation(page.url.searchParams));
	const reportId = $derived(location.reportId);
	const definitionsQuery = createListAppraisalDefinitions(() => projectId);
	const reportsQuery = createListProjectReports(
		() => projectId,
		() => ({ limit: 100 })
	);
	const documentsQuery = createListReportDocuments(
		() => projectId,
		() => reportId ?? '',
		() => ({ limit: 100 }),
		() => ({ query: { enabled: Boolean(reportId) } })
	);
	const selectedDocumentId = $derived(documentsQuery.data?.data[0]?.id);
	const blocksQuery = createListDocumentBlocks(
		() => projectId,
		() => reportId ?? '',
		() => selectedDocumentId ?? '',
		() => ({ limit: 100 }),
		() => ({ query: { enabled: Boolean(reportId && selectedDocumentId) } })
	);
	const appraisalsQuery = createListReportAppraisals(
		() => projectId,
		() => reportId ?? '',
		() => ({ query: { enabled: Boolean(reportId) } })
	);
	const completeMutation = createCompleteReportAppraisal();

	const reports = $derived(reportsQuery.data?.data.items ?? []);
	const definitions = $derived(definitionsQuery.data?.data ?? []);
	const selectedDefinition = $derived(
		definitions.find(
			(definition) =>
				definition.id === location.definitionId &&
				definition.version === location.definitionVersion
		) ?? definitions[0]
	);
	const blocks = $derived(blocksQuery.data?.data ?? []);
	const appraisals = $derived(appraisalsQuery.data?.data ?? []);

	async function selectReport(nextReportId: string): Promise<void> {
		const search = updateAppraisalLocation(page.url.searchParams, { reportId: nextReportId });
		let href: string = resolve('/projects/[projectId]/appraisal', { projectId });
		href += `?${search.toString()}`;
		await goto(href, { keepFocus: true, noScroll: true });
	}

	async function selectDefinition(definitionId: string, version: number): Promise<void> {
		const search = updateAppraisalLocation(page.url.searchParams, {
			definitionId,
			definitionVersion: version
		});
		let href: string = resolve('/projects/[projectId]/appraisal', { projectId });
		href += `?${search.toString()}`;
		await goto(href, { keepFocus: true, noScroll: true });
	}

	async function submit(request: CompleteAppraisalRequest): Promise<void> {
		if (!reportId) return;
		error = undefined;
		try {
			await completeMutation.mutateAsync({ projectId, reportId, data: request });
			await queryClient.invalidateQueries({
				queryKey: getListReportAppraisalsQueryKey(projectId, reportId)
			});
		} catch (submitError) {
			error =
				submitError instanceof Error
					? submitError.message
					: 'Appraisal could not be completed.';
			throw submitError;
		}
	}
</script>

<svelte:head>
	<title>Appraisal · DeepRef</title>
	<meta
		name="description"
		content="Complete versioned, evidence-linked appraisals using a generic renderer."
	/>
</svelte:head>

<main class="mx-auto flex max-w-7xl flex-col gap-6 p-6">
	<div>
		<p class="text-sm text-muted-foreground">Structured quality assessment</p>
		<h1 class="text-3xl font-semibold tracking-tight">Appraisal</h1>
		<p class="mt-2 max-w-3xl text-muted-foreground">
			Choose a report and definition. Suggestions guide tool choice but never complete an
			appraisal automatically.
		</p>
	</div>

	{#if error}<p
			class="rounded-md border border-destructive/50 bg-destructive/10 p-3 text-sm text-destructive"
			role="alert"
		>
			{error}
		</p>{/if}

	<div class="grid gap-6 lg:grid-cols-[18rem_18rem_1fr]">
		<Card.Root>
			<Card.Header
				><Card.Title>Report</Card.Title><Card.Description
					>URL selection is refresh-safe.</Card.Description
				></Card.Header
			>
			<Card.Content>
				<label for="appraisal-report" class="sr-only">Report to appraise</label>
				<select
					id="appraisal-report"
					class="h-9 w-full rounded-md border bg-transparent px-3 text-sm"
					value={reportId ?? ''}
					onchange={(event) => {
						if (event.currentTarget instanceof HTMLSelectElement)
							void selectReport(event.currentTarget.value);
					}}
				>
					<option value="">Select a report</option>
					{#each reports as report (report.report_id)}<option value={report.report_id}
							>{report.title ?? report.report_id}</option
						>{/each}
				</select>
			</Card.Content>
		</Card.Root>

		<Card.Root>
			<Card.Header
				><Card.Title>Definition</Card.Title><Card.Description
					>Generic API-provided schemas.</Card.Description
				></Card.Header
			>
			<Card.Content class="flex flex-col gap-2">
				{#each definitions as definition (`${definition.id}:${definition.version}`)}
					<button
						type="button"
						class="rounded-md border p-3 text-left {selectedDefinition?.id ===
							definition.id && selectedDefinition.version === definition.version
							? 'border-primary bg-muted/50'
							: ''}"
						onclick={() => void selectDefinition(definition.id, definition.version)}
					>
						<span class="block font-medium">{definition.name}</span>
						<span class="text-xs text-muted-foreground"
							>v{definition.version} · {definition.domains.length} domains</span
						>
					</button>
				{:else}<p class="text-sm text-muted-foreground">No definitions available.</p>{/each}
			</Card.Content>
		</Card.Root>

		<Card.Root>
			<Card.Header
				><Card.Title>Complete appraisal</Card.Title><Card.Description
					>Evidence selectors use parsed PR8 document blocks from the selected report.</Card.Description
				></Card.Header
			>
			<Card.Content>
				{#if !reportId}
					<p class="text-sm text-muted-foreground">Select a report to begin.</p>
				{:else if !selectedDefinition}
					<p class="text-sm text-muted-foreground">
						Select an appraisal definition to begin.
					</p>
				{:else}
					{#key `${reportId}:${selectedDefinition.id}:${selectedDefinition.version}`}
						<AppraisalForm definition={selectedDefinition} {blocks} onSubmit={submit} />
					{/key}
				{/if}
			</Card.Content>
		</Card.Root>
	</div>

	{#if reportId}
		<Card.Root>
			<Card.Header
				><Card.Title>Completed assessments</Card.Title><Card.Description
					>Immutable completion records with actor and evidence provenance.</Card.Description
				></Card.Header
			>
			<Card.Content>
				{#if appraisalsQuery.isPending}<p class="text-sm text-muted-foreground">
						Loading assessment history…
					</p>{:else if appraisals.length === 0}<p class="text-sm text-muted-foreground">
						No completed assessments for this report.
					</p>{:else}<div class="flex flex-col gap-2">
						{#each appraisals as appraisal (appraisal.id)}<div
								class="rounded-md border p-3 text-sm"
							>
								<div class="flex flex-wrap justify-between gap-2">
									<span class="font-medium"
										>{appraisal.definition_id} v{appraisal.definition_version}</span
									><span class="text-xs text-muted-foreground"
										>{new Date(appraisal.completed_at).toLocaleString()} · {appraisal.actor_id}</span
									>
								</div>
								<p class="mt-1 text-xs text-muted-foreground">
									{appraisal.evidence.length} evidence references · assessment {appraisal.id}
								</p>
							</div>{/each}
					</div>{/if}
			</Card.Content>
		</Card.Root>
	{/if}
</main>
