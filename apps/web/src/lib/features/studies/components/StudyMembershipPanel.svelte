<script lang="ts">
	import type { ReportDto, StudyDto } from '$lib/api/generated/models';
	import {
		StudyReportRoleInput,
		type StudyReportRoleInput as StudyReportRole
	} from '$lib/api/generated/models/studyReportRoleInput';
	import { Button } from '$lib/components/ui/button';
	import * as Select from '$lib/components/ui/select';

	let {
		study,
		reports,
		selectedReport,
		reportId = $bindable(),
		role = $bindable(),
		assigning,
		membershipPending,
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
		onAssign: () => void;
		onUnassign: (reportId: string) => void;
	} = $props();

	const roles = Object.values(StudyReportRoleInput);
</script>

<form
	class="grid gap-3 md:grid-cols-[1fr_12rem_auto]"
	onsubmit={(event) => {
		event.preventDefault();
		onAssign();
	}}
>
	<div class="flex flex-col gap-2">
		<label for="study-report" class="text-sm font-medium">Assign included report</label>
		<Select.Root type="single" bind:value={reportId}>
			<Select.Trigger id="study-report">
				{selectedReport?.title ?? (reportId || 'Choose report')}
			</Select.Trigger>
			<Select.Content>
				<Select.Group>
					{#each reports as report (report.report_id)}
						<Select.Item
							value={report.report_id}
							label={report.title ?? report.report_id}
						>
							{report.title ?? report.report_id}
						</Select.Item>
					{/each}
				</Select.Group>
			</Select.Content>
		</Select.Root>
	</div>
	<div class="flex flex-col gap-2">
		<label for="report-role" class="text-sm font-medium">Report role</label>
		<Select.Root type="single" bind:value={role}>
			<Select.Trigger id="report-role">{role}</Select.Trigger>
			<Select.Content>
				<Select.Group>
					{#each roles as value (value)}
						<Select.Item {value} label={value}>{value}</Select.Item>
					{/each}
				</Select.Group>
			</Select.Content>
		</Select.Root>
	</div>
	<Button type="submit" class="self-end" disabled={!reportId || assigning || membershipPending}>
		Assign / move
	</Button>
</form>

<div>
	<h2 class="text-lg font-semibold">Reports in this investigation</h2>
	<div class="mt-3 flex flex-col gap-2">
		{#each study.reports as report (report.report_id)}
			<div class="flex items-center justify-between gap-3 rounded-md border p-3">
				<div>
					<p class="font-medium">{report.title ?? report.report_id}</p>
					<p class="text-xs text-muted-foreground">{report.role} · {report.report_id}</p>
				</div>
				<Button
					type="button"
					variant="ghost"
					size="sm"
					onclick={() => onUnassign(report.report_id)}
				>
					Unassign
				</Button>
			</div>
		{:else}
			<p class="text-sm text-muted-foreground">No reports assigned yet.</p>
		{/each}
	</div>
</div>
