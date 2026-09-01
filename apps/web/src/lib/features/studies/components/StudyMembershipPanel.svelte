<script lang="ts">
	import type { ReportDto, StudyDto } from '$lib/api/generated/models';
	import {
		StudyReportRoleInput,
		type StudyReportRoleInput as StudyReportRole
	} from '$lib/api/generated/models/studyReportRoleInput';
	import { Surface } from '$lib/components/layout';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Field from '$lib/components/ui/field';
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

<Surface as="section" tone="subtle" class="flex flex-col gap-5 p-4 sm:p-5" label="Study membership">
	<div class="border-b border-border/70 pb-4">
		<div class="flex flex-wrap items-center justify-between gap-2">
			<h2 class="text-lg font-semibold">Study membership</h2>
			<Badge variant="outline">{study.reports.length} reports</Badge>
		</div>
		<p class="mt-1 text-sm text-muted-foreground">
			Assign included reports while keeping role and provenance explicit.
		</p>
	</div>
	<form
		class="grid gap-4 md:grid-cols-[minmax(0,1fr)_12rem_auto] md:items-end"
		onsubmit={(event) => {
			event.preventDefault();
			onAssign();
		}}
	>
		<Field.FieldGroup class="md:contents">
			<Field.Field>
				<Field.FieldLabel for="study-report">Assign included report</Field.FieldLabel>
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
			</Field.Field>
			<Field.Field>
				<Field.FieldLabel for="report-role">Report role</Field.FieldLabel>
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
			</Field.Field>
		</Field.FieldGroup>
		<Button type="submit" disabled={!reportId || assigning || membershipPending}>
			Assign / move
		</Button>
	</form>

	<div>
		<h3 class="text-sm font-semibold tracking-wide text-muted-foreground uppercase">
			Reports in this investigation
		</h3>
		<div class="mt-3 flex flex-col gap-2">
			{#each study.reports as report (report.report_id)}
				<Surface
					as="article"
					tone="inset"
					class="flex items-center justify-between gap-3 p-3"
					label={report.title ?? report.report_id}
				>
					<div class="min-w-0">
						<p class="truncate font-medium">{report.title ?? report.report_id}</p>
						<p class="text-xs text-muted-foreground">
							{report.role} · {report.report_id}
						</p>
					</div>
					<Button
						type="button"
						variant="ghost"
						size="sm"
						onclick={() => onUnassign(report.report_id)}
					>
						Unassign
					</Button>
				</Surface>
			{:else}
				<p class="text-sm text-muted-foreground">No reports assigned yet.</p>
			{/each}
		</div>
	</div>
</Surface>
