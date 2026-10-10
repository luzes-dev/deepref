<script lang="ts">
	import { createListDocumentBlocks } from '#lib/api/generated/documents/documents.js';
	import EvidenceLabel from '#lib/features/evidence/EvidenceLabel.svelte';
	import { citationLabel } from '#lib/features/evidence/labels.js';

	/**
	 * The text of a citation link. It quotes the cited passage, loaded from the
	 * cited document, and falls back to the page and report title while loading.
	 */
	let {
		projectId,
		reportId,
		documentId,
		blockId,
		page,
		title = null,
		technical
	}: {
		projectId: string;
		reportId: string;
		documentId: string;
		blockId: string;
		page: number;
		title?: string | null;
		/** Ids and hash for the tooltip; never shown as label text. */
		technical: string;
	} = $props();

	const blocksQuery = createListDocumentBlocks(
		() => projectId,
		() => reportId,
		() => documentId,
		() => ({ limit: 100 }),
		() => ({ query: { enabled: Boolean(projectId && reportId && documentId) } })
	);
	const passage = $derived(blocksQuery.data?.data.find((block) => block.id === blockId)?.text);
	const label = $derived(
		citationLabel({ page, quote: passage, title, fallback: 'Cited passage' })
	);
</script>

<EvidenceLabel {label} {technical} />
