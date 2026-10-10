<script lang="ts">
	import type { ReportDto, StudyDto } from '#lib/api/generated/models/index.js';
	import {
		StudyReportRoleInput,
		type StudyReportRoleInput as StudyReportRole
	} from '#lib/api/generated/models/studyReportRoleInput.js';
	import { Button } from '@deepref/ui/button';
	import * as Select from '@deepref/ui/select';

	let {
		study,
		reports,
		selectedReport,
		reportId = $bindable(),
		role = $bindable(),
		assigning,
		membershipPending,
		currentStudyLabel,
		includedReportIds,
		onAssign,
		onUnassign
	}: {
		study: StudyDto;
		reports: ReportDto[];
		selectedReport: ReportDto | undefined;
		reportId: string;
		role: StudyReportRole;
		assigning: boolean;
		membershipPending: boolean;
		/** The study the chosen report already belongs to, when adding would move it. */
		currentStudyLabel?: string;
		/** Reports included at full text; absent while unknown, in which case every report is offered. */
		includedReportIds?: ReadonlySet<string>;
		onAssign: () => void;
		onUnassign: (reportId: string) => void;
	} = $props();

	const roles = Object.values(StudyReportRoleInput);
	const memberIds = $derived(new Set(study.reports.map((report) => report.report_id)));
	let showAll = $state(false);
	const available = $derived(reports.filter((report) => !memberIds.has(report.report_id)));
	const included = $derived(
		includedReportIds
			? available.filter((report) => includedReportIds.has(report.report_id))
			: available
	);
	const others = $derived(
		includedReportIds
			? available.filter((report) => !includedReportIds.has(report.report_id))
			: []
	);
	// Studies group included reports, so those come first; the rest stay one click away.
	const hasOthers = $derived(others.length > 0);
	const expanded = $derived(showAll || (included.length === 0 && hasOthers));

	function roleLabel(value: string): string {
		return value.replaceAll('_', ' ');
	}
</script>

<section class="flex flex-col gap-4" aria-label="Study membership">
	<ol class="flex flex-col">
		{#each study.reports as report (report.report_id)}
			<li class="group flex items-center justify-between gap-3 border-b py-2.5">
				<div class="min-w-0">
					<p class="truncate text-sm font-medium">{report.title ?? report.report_id}</p>
					<p class="text-xs text-muted-foreground">{roleLabel(report.role)}</p>
				</div>
				<Button
					type="button"
					variant="ghost"
					size="sm"
					class="text-muted-foreground"
					onclick={() => onUnassign(report.report_id)}
				>
					Unassign
				</Button>
			</li>
		{:else}
			<li class="py-2 text-sm text-muted-foreground">No reports assigned yet.</li>
		{/each}
	</ol>

	<form
		class="flex flex-col gap-2"
		onsubmit={(event) => {
			event.preventDefault();
			onAssign();
		}}
	>
		<div class="flex flex-wrap items-center gap-2">
			<Select.Root type="single" bind:value={reportId}>
				<Select.Trigger id="study-report" aria-label="Report to add" class="min-w-0 flex-1">
					<span class="truncate"
						>{selectedReport?.title ?? (reportId || 'Add a report…')}</span
					>
				</Select.Trigger>
				<Select.Content>
					<Select.Group>
						{#each included as report (report.report_id)}
							<Select.Item
								value={report.report_id}
								label={report.title ?? report.report_id}
							>
								{report.title ?? report.report_id}
							</Select.Item>
						{/each}
					</Select.Group>
					{#if expanded && hasOthers}
						<Select.Group>
							<Select.GroupHeading>Not included at full text</Select.GroupHeading>
							{#each others as report (report.report_id)}
								<Select.Item
									value={report.report_id}
									label={report.title ?? report.report_id}
								>
									{report.title ?? report.report_id}
								</Select.Item>
							{/each}
						</Select.Group>
					{/if}
				</Select.Content>
			</Select.Root>
			<Select.Root type="single" bind:value={role}>
				<Select.Trigger id="report-role" aria-label="Report role" class="w-44"
					>{roleLabel(role)}</Select.Trigger
				>
				<Select.Content>
					<Select.Group>
						{#each roles as value (value)}
							<Select.Item {value} label={roleLabel(value)}
								>{roleLabel(value)}</Select.Item
							>
						{/each}
					</Select.Group>
				</Select.Content>
			</Select.Root>
			<Button type="submit" disabled={!reportId || assigning || membershipPending}>
				{currentStudyLabel ? 'Move here' : 'Add'}
			</Button>
		</div>
		{#if hasOthers && !expanded}
			<Button
				type="button"
				variant="link"
				size="xs"
				class="self-start"
				onclick={() => (showAll = true)}>Show all reports</Button
			>
		{/if}
		{#if currentStudyLabel}
			<p class="text-xs text-warning" role="status">
				This report is in “{currentStudyLabel}”. Adding it here moves it.
			</p>
		{/if}
	</form>
</section>
