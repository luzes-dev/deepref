<script lang="ts">
	import type { StudyDto } from '$lib/api/generated/models';
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';
	import { Input } from '$lib/components/ui/input';
	import { Separator } from '$lib/components/ui/separator';

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

<Card.Root>
	<Card.Header>
		<Card.Title>Study groups</Card.Title>
		<Card.Description>{studies.length} groups in this project</Card.Description>
	</Card.Header>
	<Card.Content class="flex flex-col gap-4">
		<form
			class="flex flex-col gap-2"
			onsubmit={(event) => {
				event.preventDefault();
				onCreate();
			}}
		>
			<label for="new-study-title" class="text-sm font-medium">New study title</label>
			<Input
				id="new-study-title"
				bind:value={title}
				placeholder="e.g. AMBIENT-AI Trial"
				required
			/>
			<Button type="submit" disabled={creating}>Create study</Button>
		</form>
		<Separator />
		<div class="flex flex-col gap-2" aria-live="polite">
			{#if pending}
				<p class="text-sm text-muted-foreground">Loading studies…</p>
			{:else if studies.length === 0}
				<p class="text-sm text-muted-foreground">No study groups yet.</p>
			{:else}
				{#each studies as study (study.id)}
					<button
						type="button"
						class="flex flex-col gap-1 rounded-md border p-3 text-left transition hover:bg-muted/50 {selectedStudyId ===
						study.id
							? 'border-primary bg-muted/50'
							: ''}"
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
	</Card.Content>
</Card.Root>
