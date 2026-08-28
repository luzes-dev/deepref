<script lang="ts">
	import type { PDFDocumentProxy, PDFPageProxy } from 'pdfjs-dist/types/src/display/api';
	import type { DocumentBlockDto, DocumentPageDto } from '$lib/api/generated/models';
	import PdfPage from './PdfPage.svelte';

	let {
		contentUrl,
		blocks,
		pageMetadata,
		selectedPage,
		selectedBlockId,
		onBlockSelect
	}: {
		contentUrl: string;
		blocks: DocumentBlockDto[];
		pageMetadata: DocumentPageDto[];
		selectedPage: number | null;
		selectedBlockId: string | null;
		onBlockSelect: (block: DocumentBlockDto) => void;
	} = $props();

	let container = $state<HTMLElement>();
	let pages = $state.raw<PDFPageProxy[]>([]);
	let errorMessage = $state('');
	let loading = $state(false);

	$effect(() => {
		if (!contentUrl) {
			pages = [];
			return;
		}
		let cancelled = false;
		let loadingTask:
			{ promise: Promise<PDFDocumentProxy>; destroy: () => Promise<void> } | undefined;
		loading = true;
		errorMessage = '';

		void import('pdfjs-dist')
			.then((pdfjs) => {
				if (cancelled) return undefined;
				pdfjs.GlobalWorkerOptions.workerSrc = new URL(
					'pdfjs-dist/build/pdf.worker.min.mjs',
					import.meta.url
				).toString();
				loadingTask = pdfjs.getDocument({ url: contentUrl });
				return loadingTask.promise;
			})
			.then(async (loaded) => {
				if (!loaded || cancelled) return;
				const nextPages: PDFPageProxy[] = [];
				for (let pageNumber = 1; pageNumber <= loaded.numPages; pageNumber += 1) {
					nextPages.push(await loaded.getPage(pageNumber));
				}
				if (!cancelled) pages = nextPages;
			})
			.catch((error: unknown) => {
				if (!cancelled)
					errorMessage =
						error instanceof Error ? error.message : 'The PDF could not be rendered.';
			})
			.finally(() => {
				if (!cancelled) loading = false;
			});

		return () => {
			cancelled = true;
			for (const page of pages) page.cleanup();
			void loadingTask?.destroy();
		};
	});

	$effect(() => {
		if (!container || !selectedPage) return;
		container.querySelector(`[data-page-number="${selectedPage}"]`)?.scrollIntoView({
			behavior: 'smooth',
			block: 'center'
		});
	});
</script>

<div
	bind:this={container}
	class="flex max-h-[55rem] flex-col gap-6 overflow-auto rounded-lg border bg-muted/30 p-4"
	aria-label="PDF viewer"
>
	{#if loading}
		<p class="p-8 text-center text-sm text-muted-foreground" role="status">
			Loading PDF pages…
		</p>
	{:else if errorMessage}
		<p class="p-8 text-center text-sm text-destructive" role="alert">{errorMessage}</p>
	{:else if pages.length === 0}
		<p class="p-8 text-center text-sm text-muted-foreground">No usable PDF is available yet.</p>
	{:else}
		{#each pages as page (page.pageNumber)}
			<PdfPage {page} {blocks} {pageMetadata} {selectedBlockId} {onBlockSelect} />
		{/each}
	{/if}
</div>
