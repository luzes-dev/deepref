<script lang="ts">
	import * as Field from '@deepref/ui/field';
	import * as InputGroup from '@deepref/ui/input-group';
	import { Button } from '@deepref/ui/button';
	import PlayIcon from '@lucide/svelte/icons/play';
	import { useQueryClient } from '@tanstack/svelte-query';
	import {
		getListAcquisitionsQueryKey,
		importProjectRecords
	} from '$lib/api/generated/acquisitions/acquisitions';
	import { notifyError } from '$lib/features/notifications/toast';
	import { useProjectWorkspaceContext } from '../context.svelte.js';
	import { parsePmidList, pluralize } from '../imports';

	const workspace = useProjectWorkspaceContext();
	const queryClient = useQueryClient();

	const MAX_LISTED_ISSUES = 5;
	let text = $state('');
	let submitting = $state(false);
	const parsed = $derived(parsePmidList(text));
	const canSubmit = $derived(
		parsed.pmids.length > 0 && parsed.issues.length === 0 && !submitting
	);

	/**
	 * One key per submission. A repeated list starts a new run, so a run that failed (PubMed
	 * unreachable, say) can be tried again; IDs the project already holds are reported as such.
	 * Without a key the server gives the run a fresh id, and the disabled button guards a
	 * double click.
	 */
	function submissionKey(): string | undefined {
		const id = globalThis.crypto?.randomUUID?.();
		return id ? `pmid-import:${id}` : undefined;
	}

	async function submit() {
		if (!canSubmit) return;
		submitting = true;
		const projectId = workspace.project.id;
		const content = parsed.pmids.join('\n');
		try {
			const key = submissionKey();
			const response = await importProjectRecords(
				projectId,
				{ format: 'pmid', content },
				key ? { headers: { 'Idempotency-Key': key } } : undefined
			);
			text = '';
			void queryClient.invalidateQueries({
				queryKey: getListAcquisitionsQueryKey(projectId, { strategy: 'pmid_import' })
			});
			workspace.openAcquisition(response.data.id);
		} catch (error) {
			notifyError('PubMed IDs could not be imported', error);
		} finally {
			submitting = false;
		}
	}
</script>

<form
	class="flex max-w-3xl flex-col gap-4"
	data-testid="pmid-import-form"
	onsubmit={(event) => {
		event.preventDefault();
		void submit();
	}}
>
	<Field.Field>
		<Field.FieldLabel for="pmids" class="sr-only">PubMed IDs</Field.FieldLabel>
		<InputGroup.Root>
			<InputGroup.Textarea
				id="pmids"
				rows={6}
				placeholder="One PubMed ID per line, for example 19446324. A PMID: label or a pubmed.ncbi.nlm.nih.gov link works too."
				aria-invalid={parsed.issues.length > 0 ? 'true' : undefined}
				aria-describedby="pmids-status"
				bind:value={text}
			/>
		</InputGroup.Root>
		<Field.FieldDescription>
			<span id="pmids-status" aria-live="polite">
				{#if parsed.pmids.length === 0 && parsed.issues.length === 0}
					Paste PubMed IDs, one per line or separated by commas.
				{:else}
					{pluralize(parsed.pmids.length, 'PubMed ID')} recognised{parsed.duplicates > 0
						? ` · ${parsed.duplicates} repeated ignored`
						: ''}{parsed.issues.length > 0
						? ` · ${pluralize(parsed.issues.length, 'line')} not a PubMed ID`
						: ''}
				{/if}
			</span>
		</Field.FieldDescription>
		{#if parsed.issues.length > 0}
			<ul class="flex flex-col gap-1 text-xs text-destructive" data-testid="pmid-issues">
				{#each parsed.issues.slice(0, MAX_LISTED_ISSUES) as issue (issue.line + issue.value)}
					<li>Line {issue.line}: “{issue.value}” {issue.message}</li>
				{/each}
				{#if parsed.issues.length > MAX_LISTED_ISSUES}
					<li>and {parsed.issues.length - MAX_LISTED_ISSUES} more</li>
				{/if}
			</ul>
		{/if}
	</Field.Field>

	<div class="flex flex-wrap items-center justify-between gap-4">
		<p class="max-w-md text-sm text-muted-foreground">
			DeepRef looks each ID up in PubMed in the background. Articles it finds are added as
			records, then checked for duplicates.
		</p>
		<Button type="submit" disabled={!canSubmit}>
			<PlayIcon data-icon="inline-start" />{submitting
				? 'Starting import…'
				: parsed.pmids.length > 1
					? `Import ${parsed.pmids.length} PubMed IDs`
					: 'Import PubMed ID'}
		</Button>
	</div>
</form>
