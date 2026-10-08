<script lang="ts">
	import type { ApiErrorBody, DocumentDto } from '$lib/api/generated/models';
	import { createReparseReportDocument } from '$lib/api/generated/documents/documents';
	import { ApiError } from '$lib/api/custom-fetch';
	import { Button } from '@deepref/ui/button';
	import { RefreshCw } from '@lucide/svelte';
	import { describeDocumentFailure } from '../document-failure';

	let {
		projectId,
		reportId,
		documentId,
		document,
		onFinished
	}: {
		projectId: string;
		reportId: string;
		documentId: string;
		/** The latest document state; the parent keeps it polled while reparsing. */
		document: DocumentDto | undefined;
		/** Called once a finished reparse is visible, so derived data can refresh. */
		onFinished: () => void;
	} = $props();

	const reparseMutation = createReparseReportDocument();
	/** `updated_at` as the reparse request left it; a newer value means the worker finished. */
	let baseline = $state<string | null>(null);
	let statusMessage = $state('');
	let errorMessage = $state('');
	const reparsing = $derived(baseline !== null);

	$effect(() => {
		const current = document;
		if (baseline === null || !current || current.updated_at === baseline) return;
		baseline = null;
		if (current.parser_error) {
			statusMessage = '';
			errorMessage = `Re-parse did not finish. ${describeDocumentFailure(current.parser_error).summary}`;
		} else {
			statusMessage =
				'Re-parse finished. The outline, references and evidence blocks are up to date.';
		}
		onFinished();
	});

	async function reparse() {
		if (reparsing || reparseMutation.isPending) return;
		errorMessage = '';
		statusMessage = '';
		try {
			const result = await reparseMutation.mutateAsync({ projectId, reportId, documentId });
			baseline = result.data.updated_at;
			statusMessage =
				'Re-parsing the PDF. The outline and references refresh when it finishes.';
		} catch (error) {
			errorMessage = reparseErrorMessage(error);
		}
	}

	function reparseErrorMessage(error: unknown): string {
		const code =
			error instanceof ApiError ? (error.info as ApiErrorBody | null)?.code : undefined;
		if (code === 'document_blob_missing') {
			return 'The stored PDF file is no longer on the server, so it cannot be parsed again. Upload the PDF again to re-parse it.';
		}
		if (code === 'document_not_retryable') {
			return 'This PDF cannot be re-parsed in its current state. Try again once any running processing has finished.';
		}
		return error instanceof Error ? error.message : 'Re-parse failed.';
	}
</script>

<div class="flex flex-col gap-2" data-testid="reparse-document">
	<p class="text-sm text-muted-foreground">
		Parse the stored PDF again to refresh the outline, references and evidence blocks.
	</p>
	<div class="flex flex-wrap items-center gap-3">
		<Button
			type="button"
			variant="outline"
			size="sm"
			disabled={reparsing || reparseMutation.isPending}
			onclick={() => void reparse()}
		>
			<RefreshCw data-icon="inline-start" class={reparsing ? 'animate-spin' : undefined} />
			{reparsing ? 'Re-parsing…' : 'Re-parse PDF'}
		</Button>
		{#if statusMessage}
			<p class="text-sm text-muted-foreground" role="status">{statusMessage}</p>
		{/if}
	</div>
	{#if errorMessage}
		<p class="text-sm text-destructive" role="alert">{errorMessage}</p>
	{/if}
</div>
