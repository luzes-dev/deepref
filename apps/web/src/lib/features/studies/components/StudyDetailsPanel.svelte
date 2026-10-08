<script lang="ts">
	import type { StudyDto, StudyToolSuggestionDto } from '$lib/api/generated/models';
	import { resolve } from '$app/paths';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import { Input } from '@deepref/ui/input';
	import * as Tooltip from '@deepref/ui/tooltip';
	import PencilIcon from '@lucide/svelte/icons/pencil';
	import type { Snippet } from 'svelte';
	import { SvelteURLSearchParams } from 'svelte/reactivity';
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

	let editingTitle = $state(false);
	let classifyingOpen = $state(false);

	/** Query for the appraisal form: the suggested framework and this study's first report. */
	function toolSearch(suggestion: StudyToolSuggestionDto): string {
		const params = new SvelteURLSearchParams({
			definition: suggestion.definition_id,
			definition_version: String(suggestion.definition_version)
		});
		const reportId = study.reports[0]?.report_id;
		if (reportId) params.set('report', reportId);
		return params.toString();
	}
</script>

<section class="flex min-w-0 flex-col gap-6" aria-label="Study details">
	<header class="flex flex-col gap-2">
		{#if editingTitle}
			<form
				class="flex max-w-xl gap-2"
				onsubmit={(event) => {
					event.preventDefault();
					onRename();
					editingTitle = false;
				}}
			>
				<Input
					id="rename-study-title"
					aria-label="Study title"
					bind:value={renameTitle}
					placeholder={study.title}
					required
				/>
				<Button type="submit" disabled={renaming || !renameTitle.trim()}>Save</Button>
				<Button type="button" variant="ghost" onclick={() => (editingTitle = false)}
					>Cancel</Button
				>
			</form>
		{:else}
			<div class="flex items-start gap-1">
				<h2 class="editorial-title text-xl">{study.title}</h2>
				<Button
					variant="ghost"
					size="icon-xs"
					aria-label="Rename study"
					onclick={() => {
						renameTitle = study.title;
						editingTitle = true;
					}}><PencilIcon aria-hidden="true" /></Button
				>
			</div>
		{/if}
		<div class="flex flex-wrap items-center gap-x-3 gap-y-1 text-sm text-muted-foreground">
			<span>{study.design_label ?? 'Design not classified'}</span>
			<Button
				variant="link"
				size="xs"
				aria-expanded={classifyingOpen}
				onclick={() => (classifyingOpen = !classifyingOpen)}
				>{study.design_label ? 'Change' : 'Classify'}</Button
			>
			{#if study.tool_suggestions.length > 0}
				<span aria-hidden="true">·</span>
				<Tooltip.Provider>
					{#each study.tool_suggestions as suggestion (suggestion.tool)}
						<Tooltip.Root>
							<Tooltip.Trigger>
								{#snippet child({ props })}
									<a
										{...props}
										href={resolve(
											`/projects/${encodeURIComponent(study.project_id)}/appraisal?${toolSearch(suggestion)}`
										)}
										class="rounded-md outline-none focus-visible:ring-3 focus-visible:ring-ring/30"
										data-testid="suggested-tool-link"
									>
										<Badge size="sm" variant="outline"
											>Suggested tool · {suggestion.tool}</Badge
										>
									</a>
								{/snippet}
							</Tooltip.Trigger>
							<Tooltip.Content class="max-w-xs"
								>{suggestion.rationale}</Tooltip.Content
							>
						</Tooltip.Root>
					{/each}
				</Tooltip.Provider>
			{/if}
		</div>
		{#if classifyingOpen}
			<div class="max-w-md pt-2">
				{#key `${study.id}:${study.revision}`}
					<StudyClassificationForm
						{study}
						{designs}
						disabled={classifying}
						onSubmit={async (request) => {
							await onClassify(request);
							classifyingOpen = false;
						}}
					/>
				{/key}
			</div>
		{/if}
	</header>
	{@render children()}
</section>
