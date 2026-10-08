<script lang="ts">
	import type { StudyDto, UngroupedReportDto } from '$lib/api/generated/models';
	import { Button } from '@deepref/ui/button';
	import { Input } from '@deepref/ui/input';
	import { Skeleton } from '@deepref/ui/skeleton';
	import PlusIcon from '@lucide/svelte/icons/plus';

	let {
		studies,
		selectedStudyId,
		pending,
		creating,
		title = $bindable(),
		ungrouped,
		ungroupedPending,
		groupingReportId,
		onCreate,
		onSelect,
		onCreateFromReport
	}: {
		studies: StudyDto[];
		selectedStudyId: string | undefined;
		pending: boolean;
		creating: boolean;
		title: string;
		ungrouped: UngroupedReportDto[];
		ungroupedPending: boolean;
		groupingReportId: string | null;
		onCreate: () => void;
		onSelect: (studyId: string) => void;
		onCreateFromReport: (report: UngroupedReportDto) => void;
	} = $props();

	let composing = $state(false);
</script>

<section class="flex h-full min-h-0 flex-col" aria-label="Study groups">
	<div class="flex items-center justify-between gap-2 px-4 pt-4 pb-2">
		<h2 class="text-sm font-semibold">
			Studies <span class="font-normal text-muted-foreground tabular-nums"
				>{studies.length}</span
			>
		</h2>
		<Button
			variant="ghost"
			size="xs"
			aria-expanded={composing}
			onclick={() => (composing = !composing)}
			><PlusIcon data-icon="inline-start" />New</Button
		>
	</div>
	{#if composing || (!pending && studies.length === 0)}
		<form
			class="flex gap-2 px-4 pb-3"
			onsubmit={(event) => {
				event.preventDefault();
				onCreate();
				composing = false;
			}}
		>
			<Input
				id="new-study-title"
				aria-label="New study title"
				class="h-8"
				bind:value={title}
				placeholder="Study name, e.g. AMBIENT-AI trial"
				required
			/>
			<Button type="submit" size="sm" disabled={creating || !title.trim()}>Create</Button>
		</form>
	{/if}
	<div class="min-h-0 flex-1 overflow-y-auto" aria-live="polite">
		{#if pending}
			<div class="flex flex-col gap-2 p-2">
				{#each { length: 3 }, index (index)}<Skeleton class="h-12 w-full" />{/each}
			</div>
		{:else if studies.length === 0}
			<p class="px-4 text-sm text-muted-foreground">
				A study groups the reports from one investigation. Name the first one above.
			</p>
		{:else}
			<ol class="flex flex-col p-1.5">
				{#each studies as study (study.id)}
					{@const selected = selectedStudyId === study.id}
					<li>
						<button
							type="button"
							class={[
								'flex w-full flex-col gap-0.5 rounded-md px-2.5 py-2 text-left text-sm transition-colors hover:bg-muted focus-visible:outline-2 focus-visible:outline-ring',
								selected && 'bg-accent shadow-inset-accent'
							]}
							data-selected={selected}
							aria-current={selected ? 'true' : undefined}
							onclick={() => onSelect(study.id)}
						>
							<span class={['leading-snug', selected && 'font-medium']}
								>{study.title}</span
							>
							<span class="text-xs text-muted-foreground"
								>{study.design_label ?? 'Not classified'}</span
							>
						</button>
					</li>
				{/each}
			</ol>
		{/if}
		<section
			class="border-t border-border px-2.5 py-3"
			aria-label="Included reports without a study"
		>
			<h3 class="px-1.5 pb-2 text-xs font-semibold text-muted-foreground">
				Included, not in a study
				<span class="font-normal tabular-nums"
					>{ungroupedPending ? '' : ungrouped.length}</span
				>
			</h3>
			{#if ungroupedPending}
				<div class="flex flex-col gap-2 px-1.5">
					<Skeleton class="h-10 w-full" />
				</div>
			{:else if ungrouped.length === 0}
				<p class="px-1.5 text-xs text-muted-foreground">
					Every included report belongs to a study.
				</p>
			{:else}
				<ul class="flex flex-col gap-1">
					{#each ungrouped as report (report.report_id)}
						{@const details = [report.doi, report.publication_year, report.journal]
							.filter(Boolean)
							.join(' · ')}
						<li class="flex items-start justify-between gap-2 rounded-md px-1.5 py-1.5">
							<div class="flex min-w-0 flex-col gap-0.5">
								<span class="text-sm leading-snug break-words">
									{report.title ?? 'Untitled report'}
								</span>
								<span class="text-xs text-muted-foreground">
									{details || 'No DOI, year or journal recorded'}
								</span>
							</div>
							<Button
								variant="outline"
								size="xs"
								class="shrink-0"
								disabled={groupingReportId !== null}
								aria-label={`New study from ${report.title ?? 'this report'}`}
								onclick={() => onCreateFromReport(report)}
							>
								{groupingReportId === report.report_id ? 'Creating…' : 'New study'}
							</Button>
						</li>
					{/each}
				</ul>
			{/if}
		</section>
	</div>
</section>
