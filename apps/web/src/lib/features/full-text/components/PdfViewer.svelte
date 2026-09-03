<script lang="ts">
	import type { PDFDocumentProxy, PDFPageProxy } from 'pdfjs-dist/types/src/display/api';
	import type { DocumentBlockDto, DocumentPageDto } from '$lib/api/generated/models';
	import * as Alert from '$lib/components/ui/alert';
	import * as Empty from '$lib/components/ui/empty';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { FileWarning } from '@lucide/svelte';
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
	class="flex max-h-[55rem] min-h-[24rem] flex-col gap-6 overflow-auto rounded-xl border bg-muted/30 p-4 shadow-inner sm:p-6"
	aria-label="PDF viewer"
	data-testid="pdf-viewer"
>
	{#if loading}
		<div class="flex flex-col gap-3 p-8" role="status" aria-label="Loading PDF pages">
			<Skeleton class="mx-auto h-8 w-40" />
			<Skeleton class="mx-auto h-[28rem] w-full max-w-2xl" />
			<p class="text-center text-sm text-muted-foreground">Loading PDF pages…</p>
		</div>
	{:else if errorMessage}
		<Alert.Root variant="destructive" class="m-4">
			<FileWarning aria-hidden="true" />
			<Alert.Title>PDF could not be rendered</Alert.Title>
			<Alert.Description>{errorMessage}</Alert.Description>
		</Alert.Root>
	{:else if pages.length === 0}
		<Empty.Root class="min-h-[22rem] border-dashed">
			<Empty.Media variant="icon"><FileWarning /></Empty.Media>
			<Empty.Header>
				<Empty.Title>No usable PDF is available yet</Empty.Title>
				<Empty.Description
					>Attach a PDF or wait for parsing to finish before reviewing evidence blocks.</Empty.Description
				>
			</Empty.Header>
		</Empty.Root>
	{:else}
		{#each pages as page (page.pageNumber)}
			<PdfPage {page} {blocks} {pageMetadata} {selectedBlockId} {onBlockSelect} />
		{/each}
	{/if}
</div>
