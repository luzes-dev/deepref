<script lang="ts">
	import type { StudyDto } from '$lib/api/generated/models';
	import { Button } from '$lib/components/ui/button';
	import * as Field from '$lib/components/ui/field';
	import { Input } from '$lib/components/ui/input';
	import { Separator } from '$lib/components/ui/separator';
	import { StatePanel, Surface } from '$lib/components/layout';
	import { cn } from '$lib/utils';

	let {
		studies,
		selectedStudyId,
		pending,
		creating,
		title = $bindable(),
		onCreate,
		onSelect
	}: {
		studies: StudyDto[];
		selectedStudyId: string | undefined;
		pending: boolean;
		creating: boolean;
		title: string;
		onCreate: () => void;
		onSelect: (studyId: string) => void;
	} = $props();
</script>

<Surface
	as="section"
	tone="default"
	class="flex min-h-0 flex-col gap-4 p-4 sm:p-5"
	label="Study groups"
>
	<div class="border-b border-border/70 pb-4">
		<h2 class="text-lg font-semibold">Study groups</h2>
		<p class="mt-1 text-sm text-muted-foreground">
			{studies.length}
			{studies.length === 1 ? 'group' : 'groups'} in this project
		</p>
	</div>
	<div class="flex flex-col gap-4">
		<form
			class="flex flex-col gap-2"
			onsubmit={(event) => {
				event.preventDefault();
				onCreate();
			}}
		>
			<Field.FieldGroup>
				<Field.Field>
					<Field.FieldLabel for="new-study-title">New study title</Field.FieldLabel>
					<Input
						id="new-study-title"
						bind:value={title}
						placeholder="e.g. AMBIENT-AI Trial"
						required
					/>
				</Field.Field>
			</Field.FieldGroup>
			<Button type="submit" disabled={creating}>Create study</Button>
		</form>
		<Separator />
		<div class="flex flex-col gap-2" aria-live="polite">
			{#if pending}
				<StatePanel
					state="loading"
					title="Loading study groups"
					description="Retrieving this project's study groups."
				/>
			{:else if studies.length === 0}
				<StatePanel
					state="empty"
					title="No study groups yet."
					description="Create a group to organize reports from one investigation."
				/>
			{:else}
				{#each studies as study (study.id)}
					<button
						type="button"
						class={cn(
							'flex flex-col gap-1 rounded-lg border p-3 text-left transition-colors hover:bg-muted/50',
							selectedStudyId === study.id &&
								'border-primary bg-primary/5 ring-1 ring-primary/30'
						)}
						data-selected={selectedStudyId === study.id}
						aria-current={selectedStudyId === study.id ? 'true' : undefined}
						onclick={() => onSelect(study.id)}
					>
						<span class="font-medium">{study.title}</span>
						<span class="text-xs text-muted-foreground">
							Open to inspect membership · revision {study.revision}
						</span>
					</button>
				{/each}
			{/if}
		</div>
	</div>
</Surface>
