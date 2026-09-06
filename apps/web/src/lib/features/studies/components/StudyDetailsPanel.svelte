<script lang="ts">
	import type { StudyDto } from '$lib/api/generated/models';
	import { MetricTile, Surface } from '$lib/components/layout';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Field from '$lib/components/ui/field';
	import { Input } from '$lib/components/ui/input';
	import type { Snippet } from 'svelte';
	import StudyClassificationForm from './StudyClassificationForm.svelte';

	type ClassificationRequest = {
		design: string;
		physiotherapy: boolean;
		exposure: boolean;
		prediction_or_ai: boolean;
	};

	let {
		study,
		designs,
		renameTitle = $bindable(),
		renaming,
		classifying,
		onRename,
		onClassify,
		children
	}: {
		study: StudyDto;
		designs: readonly { value: string; label: string }[];
		renameTitle: string;
		renaming: boolean;
		classifying: boolean;
		onRename: () => void;
		onClassify: (request: ClassificationRequest) => Promise<void>;
		children: Snippet;
	} = $props();
</script>

<Surface as="section" tone="default" class="flex min-w-0 flex-col" label="Study details">
	<div class="border-b border-border/70 p-4 sm:p-5">
		<div class="flex flex-wrap items-start justify-between gap-3">
			<div>
				<h2 class="text-xl font-semibold tracking-tight">{study.title}</h2>
				<p class="mt-1 text-sm text-muted-foreground">
					Revision {study.revision} · changes are audited and reversible
				</p>
			</div>
			{#if study.design_label}
				<Badge variant="secondary">{study.design_label}</Badge>
			{/if}
		</div>
	</div>
	<div class="flex flex-col gap-6 p-4 sm:p-5">
		<div class="grid gap-3 sm:grid-cols-3">
			<MetricTile
				label="Reports"
				value={study.reports.length}
				detail="in this investigation"
			/>
			<MetricTile
				label="Revision"
				value={study.revision}
				detail="audited changes"
				tone="info"
			/>
			<MetricTile
				label="Design"
				value={study.design_label ?? 'Unclassified'}
				detail={study.design ? 'normalized' : 'needs review'}
				tone={study.design ? 'positive' : 'warning'}
			/>
		</div>
		<div class="grid gap-5 border-t border-border/70 pt-5 lg:grid-cols-3">
			<form
				class="flex flex-col gap-2"
				onsubmit={(event) => {
					event.preventDefault();
					onRename();
				}}
			>
				<Field.FieldGroup>
					<Field.Field>
						<Field.FieldLabel for="rename-study-title">Rename</Field.FieldLabel>
						<Input
							id="rename-study-title"
							bind:value={renameTitle}
							placeholder={study.title}
							required
						/>
					</Field.Field>
				</Field.FieldGroup>
				<Button type="submit" variant="outline" disabled={renaming}>Save title</Button>
			</form>
			{#key `${study.id}:${study.revision}`}
				<StudyClassificationForm
					{study}
					{designs}
					disabled={classifying}
					onSubmit={onClassify}
				/>
			{/key}
			<Surface as="aside" tone="inset" class="p-4" label="Suggested tools">
				<p class="font-medium">Suggested tools</p>
				<p class="mt-1 text-xs text-muted-foreground">
					Guidance only; never completes an appraisal automatically.
				</p>
				<div class="mt-3 flex flex-wrap gap-2">
					{#each study.tool_suggestions as suggestion (suggestion.tool)}
						<Badge variant="outline" title={suggestion.rationale}
							>{suggestion.tool}</Badge
						>
					{/each}
				</div>
			</Surface>
		</div>
		{@render children()}
	</div>
</Surface>
