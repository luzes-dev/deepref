<script lang="ts">
	import type { PDFPageProxy } from 'pdfjs-dist/types/src/display/api';
	import type { DocumentBlockDto, DocumentPageDto } from '$lib/api/generated/models';
	import EvidenceOverlay from './EvidenceOverlay.svelte';

	let {
		page,
		blocks,
		pageMetadata,
		selectedBlockId,
		onBlockSelect,
		onPageSize
	}: {
		page: PDFPageProxy;
		blocks: DocumentBlockDto[];
		pageMetadata: DocumentPageDto[];
		selectedBlockId: string | null;
		onBlockSelect: (block: DocumentBlockDto) => void;
		onPageSize?: (width: number, height: number) => void;
	} = $props();

	let canvas = $state<HTMLCanvasElement>();
	let viewportSize = $state({ width: 0, height: 0 });
	const pageBlocks = $derived(blocks.filter((block) => block.page_number === page.pageNumber));
	const pageInfo = $derived(pageMetadata.find((item) => item.page_number === page.pageNumber));

	$effect(() => {
		if (!canvas) return;
		const viewport = page.getViewport({ scale: 1.35 });
		viewportSize = { width: viewport.width, height: viewport.height };
		onPageSize?.(viewport.width, viewport.height);
		canvas.width = viewport.width;
		canvas.height = viewport.height;
		canvas.style.aspectRatio = `${viewport.width} / ${viewport.height}`;
		const context = canvas.getContext('2d');
		if (!context) return;
		const renderTask = page.render({ canvas, canvasContext: context, viewport });
		return () => renderTask.cancel();
	});
</script>

<div
	class="relative w-fit shrink-0 self-start bg-white shadow-sm"
	style:width={`${viewportSize.width}px`}
	style:height={`${viewportSize.height}px`}
	data-page-number={page.pageNumber}
	data-ocr-required={pageInfo?.ocr_required ? 'true' : 'false'}
>
	<canvas bind:this={canvas} class="block" aria-label={`PDF page ${page.pageNumber}`}></canvas>
	{#each pageBlocks as block (block.id)}
		<EvidenceOverlay
			{block}
			pageWidth={viewportSize.width}
			pageHeight={viewportSize.height}
			selected={selectedBlockId === block.id}
			onSelect={onBlockSelect}
		/>
	{/each}
</div>
