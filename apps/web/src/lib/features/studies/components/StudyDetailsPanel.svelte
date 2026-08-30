<script lang="ts">
	import type { StudyDto } from '$lib/api/generated/models';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';
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

<Card.Root>
	<Card.Header>
		<div class="flex flex-wrap items-start justify-between gap-3">
			<div>
				<Card.Title>{study.title}</Card.Title>
				<Card.Description>
					Revision {study.revision} · changes are audited and reversible
				</Card.Description>
			</div>
			{#if study.design_label}
				<Badge variant="secondary">{study.design_label}</Badge>
			{/if}
		</div>
	</Card.Header>
	<Card.Content class="flex flex-col gap-6">
		<div class="grid gap-4 md:grid-cols-3">
			<form
				class="flex flex-col gap-2"
				onsubmit={(event) => {
					event.preventDefault();
					onRename();
				}}
			>
				<label for="rename-study-title" class="text-sm font-medium">Rename</label>
				<Input
					id="rename-study-title"
					bind:value={renameTitle}
					placeholder={study.title}
					required
				/>
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
			<div class="rounded-md border bg-muted/20 p-3 text-sm">
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
			</div>
		</div>
		{@render children()}
	</Card.Content>
</Card.Root>
