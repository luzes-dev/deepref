<script lang="ts">
	import type { DocumentBlockDto } from '$lib/api/generated/models';
	import { normalizedBboxToPixels, type NormalizedBbox } from '../bbox';

	let {
		block,
		pageWidth,
		pageHeight,
		selected = false,
		onSelect
	}: {
		block: DocumentBlockDto;
		pageWidth: number;
		pageHeight: number;
		selected?: boolean;
		onSelect: (block: DocumentBlockDto) => void;
	} = $props();

	function normalizedBbox(value: unknown): NormalizedBbox | null {
		if (!value || typeof value !== 'object') return null;
		if (!('x' in value) || !('y' in value) || !('width' in value) || !('height' in value))
			return null;
		const candidate = value;
		return typeof candidate.x === 'number' &&
			typeof candidate.y === 'number' &&
			typeof candidate.width === 'number' &&
			typeof candidate.height === 'number'
			? { x: candidate.x, y: candidate.y, width: candidate.width, height: candidate.height }
			: null;
	}

	const pixels = $derived(
		normalizedBboxToPixels(normalizedBbox(block.bbox), pageWidth, pageHeight)
	);
</script>

{#if pixels}
	<button
		type="button"
		class="absolute rounded-sm border-2 transition-colors focus-visible:ring-2 focus-visible:ring-ring focus-visible:outline-none {selected
			? 'border-primary bg-primary/25'
			: 'border-primary/50 bg-primary/5 hover:bg-primary/20'}"
		style:left={`${pixels.x}px`}
		style:top={`${pixels.y}px`}
		style:width={`${pixels.width}px`}
		style:height={`${pixels.height}px`}
		aria-label={`Evidence block on page ${block.page_number}`}
		aria-pressed={selected}
		onclick={() => onSelect(block)}
	></button>
{/if}
