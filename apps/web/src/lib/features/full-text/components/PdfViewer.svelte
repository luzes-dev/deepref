<script lang="ts">
	import type { PDFDocumentProxy, PDFPageProxy } from 'pdfjs-dist/types/src/display/api';
	import type { DocumentBlockDto, DocumentPageDto } from '$lib/api/generated/models';
	import * as Alert from '@deepref/ui/alert';
	import * as Empty from '@deepref/ui/empty';
	import { Skeleton } from '@deepref/ui/skeleton';
	import { Button } from '@deepref/ui/button';
	import { FileWarning, Minus, Plus, Scan } from '@lucide/svelte';
	import PdfPage from './PdfPage.svelte';

	let {
		contentUrl,
		blocks,
		pageMetadata,
		selectedPage,
		selectedBlockId,
		onBlockSelect,
		fill = false
	}: {
		contentUrl: string;
		blocks: DocumentBlockDto[];
		pageMetadata: DocumentPageDto[];
		selectedPage: number | null;
		selectedBlockId: string | null;
		onBlockSelect: (block: DocumentBlockDto) => void;
		/** Fill the parent pane instead of rendering as a bounded inset viewer. */
		fill?: boolean;
	} = $props();

	let container = $state<HTMLElement>();
	let pages = $state.raw<PDFPageProxy[]>([]);
	let errorMessage = $state('');
	let loading = $state(false);
	let viewerWidth = $state(0);
	let zoom = $state(1);

	const MIN_ZOOM = 0.5;
	const MAX_ZOOM = 3;
	// Padding on each side of the page inside the scroller (p-4 / sm:p-6).
	const pageGutter = $derived(viewerWidth >= 640 ? 48 : 32);
	const fitWidth = $derived(Math.max(0, viewerWidth - pageGutter));
	const pageWidth = $derived(fitWidth * zoom);

	function zoomBy(factor: number) {
		zoom = Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, Math.round(zoom * factor * 100) / 100));
	}

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

	// Overlays and pages size themselves after they mount, so the target is
	// looked up on animation frames until it has a size, then centred.
	const MAX_CENTER_FRAMES = 120;
	// The block the reader picked with a click. It is already on screen, so it is
	// not re-centred: that would jump the page under the pointer.
	let pickedBlockId: string | null = null;

	function pickBlock(block: DocumentBlockDto) {
		pickedBlockId = block.id;
		onBlockSelect(block);
	}

	function centerElement(element: HTMLElement) {
		if (!container) return;
		// On a phone the inset viewer can sit below the fold: bring it into the
		// window first, so the block is visible to the reader and not only scrolled
		// inside an off-screen box.
		const frame = container.getBoundingClientRect();
		if (frame.top < 0 || frame.bottom > window.innerHeight) {
			container.scrollIntoView({ block: 'center', behavior: 'instant' });
		}
		if (container.scrollHeight > container.clientHeight) {
			const viewport = container.getBoundingClientRect();
			const box = element.getBoundingClientRect();
			const top =
				container.scrollTop +
				(box.top - viewport.top) -
				(container.clientHeight - box.height) / 2;
			container.scrollTo({ top: Math.max(0, top), behavior: 'smooth' });
		} else {
			element.scrollIntoView({ behavior: 'smooth', block: 'center' });
		}
	}

	$effect(() => {
		const blockId = selectedBlockId;
		const pageNumber = selectedPage;
		const renderedPages = pages.length;
		void blocks.length; // re-centre once the evidence overlays arrive
		// Consume the click marker, so only the selection that came from a click is skipped.
		const pickedByClick = blockId !== null && blockId === pickedBlockId;
		pickedBlockId = null;
		if (!container || renderedPages === 0 || (!blockId && !pageNumber)) return;
		let frames = 0;
		let frame = 0;
		const center = () => {
			const overlay = blockId
				? container?.querySelector<HTMLElement>('[data-selected="true"]')
				: null;
			if (overlay && overlay.getBoundingClientRect().height > 0) {
				if (!pickedByClick) centerElement(overlay);
				return;
			}
			if (frames < MAX_CENTER_FRAMES) {
				frames += 1;
				frame = requestAnimationFrame(center);
				return;
			}
			const page = container?.querySelector<HTMLElement>(
				`[data-page-number="${pageNumber}"]`
			);
			if (page) centerElement(page);
		};
		frame = requestAnimationFrame(center);
		return () => cancelAnimationFrame(frame);
	});
</script>

<div class={['relative', fill ? 'h-full min-h-0' : '']}>
	<div
		bind:this={container}
		bind:clientWidth={viewerWidth}
		class={[
			'flex flex-col items-start gap-6 overflow-auto bg-muted/30 p-4 sm:p-6',
			fill ? 'h-full min-h-0' : 'max-h-[55rem] min-h-[24rem] rounded-xl border shadow-inner'
		]}
		role="region"
		aria-label="PDF viewer"
		data-testid="pdf-viewer"
	>
		{#if loading}
			<div
				class="flex w-full flex-col gap-3 p-8"
				role="status"
				aria-label="Loading PDF pages"
			>
				<Skeleton class="mx-auto h-8 w-40" />
				<Skeleton class="mx-auto h-[28rem] w-full max-w-2xl" />
				<p class="text-center text-sm text-muted-foreground">Loading PDF pages…</p>
			</div>
		{:else if errorMessage}
			<Alert.Root variant="destructive" class="m-4 w-auto self-stretch">
				<FileWarning aria-hidden="true" />
				<Alert.Title>PDF could not be rendered</Alert.Title>
				<Alert.Description>
					If the stored file was lost, upload the PDF again from Replace PDF. The parsed
					text stays available under Parsed evidence.
					<span class="block pt-1 text-xs opacity-80"
						>Technical detail: {errorMessage}</span
					>
				</Alert.Description>
			</Alert.Root>
		{:else if pages.length === 0}
			<Empty.Root class="min-h-[22rem] w-full border-dashed">
				<Empty.Media variant="icon"><FileWarning /></Empty.Media>
				<Empty.Header>
					<Empty.Title>No usable PDF is available yet</Empty.Title>
					<Empty.Description
						>Attach a PDF or wait for parsing to finish before reviewing evidence
						blocks.</Empty.Description
					>
				</Empty.Header>
			</Empty.Root>
		{:else}
			{#each pages as page (page.pageNumber)}
				<PdfPage
					{page}
					{blocks}
					{pageMetadata}
					{selectedBlockId}
					onBlockSelect={pickBlock}
					width={pageWidth}
				/>
			{/each}
		{/if}
	</div>
	{#if pages.length > 0}
		<div
			class="absolute top-3 right-3 flex items-center gap-0.5 rounded-md border bg-card/90 p-0.5 shadow-sm backdrop-blur-xs"
			role="group"
			aria-label="Zoom"
		>
			<Button
				variant="ghost"
				size="icon-xs"
				aria-label="Zoom out"
				disabled={zoom <= MIN_ZOOM}
				onclick={() => zoomBy(1 / 1.25)}><Minus aria-hidden="true" /></Button
			>
			<Button
				variant="ghost"
				size="icon-xs"
				aria-label="Fit to width"
				disabled={zoom === 1}
				onclick={() => (zoom = 1)}><Scan aria-hidden="true" /></Button
			>
			<Button
				variant="ghost"
				size="icon-xs"
				aria-label="Zoom in"
				disabled={zoom >= MAX_ZOOM}
				onclick={() => zoomBy(1.25)}><Plus aria-hidden="true" /></Button
			>
		</div>
	{/if}
</div>
