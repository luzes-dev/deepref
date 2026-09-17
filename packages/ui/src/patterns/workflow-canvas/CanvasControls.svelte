<script lang="ts">
	import { Panel, useSvelteFlow, useViewport } from "@xyflow/svelte";
	import { prefersReducedMotion } from "svelte/motion";
	import MinusIcon from "@lucide/svelte/icons/minus";
	import PlusIcon from "@lucide/svelte/icons/plus";
	import Maximize2Icon from "@lucide/svelte/icons/maximize-2";
	import PlayIcon from "@lucide/svelte/icons/play";
	import ClockIcon from "@lucide/svelte/icons/clock";
	import SparklesIcon from "@lucide/svelte/icons/sparkles";
	import SlidersHorizontalIcon from "@lucide/svelte/icons/sliders-horizontal";
	import {
		WORKFLOW_FIT_VIEW_OPTIONS,
		WORKFLOW_ZOOM_LIMITS,
	} from "./viewport-options";

	let {
		onRunWorkflow,
		onShowHistory,
		onAutoLayout,
	}: {
		onRunWorkflow?: () => void;
		onShowHistory?: () => void;
		onAutoLayout?: () => void;
	} = $props();

	const { zoomIn, zoomOut, fitView, setZoom } = useSvelteFlow();
	const viewport = useViewport();

	const currentZoom = $derived(Math.round(viewport.current.zoom * 100));
	const sliderMin = Math.round(WORKFLOW_ZOOM_LIMITS.minZoom * 100);
	const sliderMax = Math.round(WORKFLOW_ZOOM_LIMITS.maxZoom * 100);
	const sliderZoom = $derived(
		Math.min(sliderMax, Math.max(sliderMin, currentZoom)),
	);
	const interactionDuration = $derived(
		prefersReducedMotion.current ? 0 : 200,
	);
	const fitDuration = $derived(prefersReducedMotion.current ? 0 : 300);

	function handleZoomIn(): void {
		void zoomIn({ duration: interactionDuration });
	}

	function handleZoomOut(): void {
		void zoomOut({ duration: interactionDuration });
	}

	function handleFitView(): void {
		void fitView({ ...WORKFLOW_FIT_VIEW_OPTIONS, duration: fitDuration });
	}

	function handleSliderChange(event: Event): void {
		const target = event.target as HTMLInputElement;
		const val = parseFloat(target.value) / 100;
		void setZoom(val, { duration: prefersReducedMotion.current ? 0 : 150 });
	}
</script>

<!-- Bottom-Left Floating Zoom Bar -->
<Panel position="bottom-left" class="!m-2 select-none sm:!m-4">
	<div
		class="flex items-center gap-1.5 rounded-full border border-border bg-card px-2.5 py-1.5 text-foreground shadow-none sm:gap-2.5 sm:px-3.5"
	>
		<!-- Zoom Out -->
		<button
			type="button"
			class="flex size-7 cursor-pointer items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-primary/20 hover:text-foreground sm:size-6"
			onclick={handleZoomOut}
			aria-label="Zoom out"
		>
			<MinusIcon class="size-3.5" />
		</button>

		<!-- Compact zoom slider -->
		<div class="relative flex w-24 items-center sm:w-32">
			<input
				type="range"
				aria-label="Zoom"
				min={sliderMin}
				max={sliderMax}
				value={sliderZoom}
				oninput={handleSliderChange}
				class="h-1.5 w-full cursor-pointer appearance-none rounded-lg bg-muted accent-primary focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
			/>
		</div>

		<!-- Zoom In -->
		<button
			type="button"
			class="flex size-7 cursor-pointer items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-primary/20 hover:text-foreground sm:size-6"
			onclick={handleZoomIn}
			aria-label="Zoom in"
		>
			<PlusIcon class="size-3.5" />
		</button>

		<!-- Zoom Percentage Display -->
		<span
			class="min-w-8 text-center font-mono text-xs font-semibold text-foreground sm:min-w-10"
		>
			{currentZoom}%
		</span>

		<!-- Divider -->
		<div class="h-3.5 w-px bg-border"></div>

		<!-- Fit View Button -->
		<button
			type="button"
			class="flex size-7 cursor-pointer items-center justify-center rounded-full text-muted-foreground transition-colors hover:bg-primary/20 hover:text-foreground sm:size-6"
			onclick={handleFitView}
			aria-label="Fit to view"
		>
			<Maximize2Icon class="size-3.5" />
		</button>
	</div>
</Panel>

<!-- Vertical Floating Rail -->
<Panel position="top-right" class="!m-2 select-none sm:!m-4">
	<div
		class="flex flex-col items-center gap-1 rounded-lg border border-border bg-card p-1.5 shadow-none"
	>
		{#if onRunWorkflow}
			<button
				type="button"
				class="flex size-8 cursor-pointer items-center justify-center rounded-lg text-success transition-colors hover:bg-success/20"
				onclick={onRunWorkflow}
				aria-label="Execute flow"
			>
				<PlayIcon class="size-4 fill-current" />
			</button>
		{/if}

		{#if onShowHistory}
			<button
				type="button"
				class="flex size-8 cursor-pointer items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-primary/20 hover:text-primary"
				onclick={onShowHistory}
				aria-label="Run history and timeline"
			>
				<ClockIcon class="size-4" />
			</button>
		{/if}

		{#if onAutoLayout}
			<button
				type="button"
				class="flex size-8 cursor-pointer items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-primary/20 hover:text-primary"
				onclick={onAutoLayout}
				aria-label="Auto-arrange nodes"
			>
				<SparklesIcon class="size-4" />
			</button>
		{/if}

		<button
			type="button"
			class="flex size-8 cursor-pointer items-center justify-center rounded-lg text-muted-foreground transition-colors hover:bg-primary/20 hover:text-primary"
			onclick={handleFitView}
			aria-label="Center view"
		>
			<SlidersHorizontalIcon class="size-4" />
		</button>

		<div class="my-1 h-px w-4 bg-card"></div>
	</div>
</Panel>
