<script lang="ts">
	import { onMount } from 'svelte';
	import { Button } from '@deepref/ui/button';
	import { Spinner } from '@deepref/ui/spinner';
	import { DEFAULT_WORKFLOW_REGISTRY, type WorkflowNodeRegistry } from '../domain/registry';
	import type { WorkflowDefinition, WorkflowNodeId } from '../domain/types';
	import type {
		WorkflowEditorChange,
		WorkflowEditorError,
		WorkflowEditorHandle
	} from './rete-adapter';

	let {
		workflow,
		registry = DEFAULT_WORKFLOW_REGISTRY,
		readOnly = false,
		onChange,
		onSelectionChange,
		onError,
		onReady,
		controller = $bindable<WorkflowEditorHandle | null>(null)
	}: {
		workflow: WorkflowDefinition;
		registry?: WorkflowNodeRegistry;
		readOnly?: boolean;
		onChange?: (change: WorkflowEditorChange) => void;
		onSelectionChange?: (ids: readonly WorkflowNodeId[]) => void;
		onError?: (error: WorkflowEditorError) => void;
		onReady?: (controller: WorkflowEditorHandle) => void;
		controller?: WorkflowEditorHandle | null;
	} = $props();

	let container: HTMLDivElement;
	let ready = $state(false);
	let failure = $state('');
	let zoomPercent = $state(100);
	let selected: readonly WorkflowNodeId[] = [];
	let hadNodes = false;
	let disposed = false;
	let reconciliation = Promise.resolve();
	const instanceId = $props.id();
	const hintId = `${instanceId}-hint`;

	function report(error: unknown): void {
		if (disposed) return;
		failure =
			error instanceof Error ? error.message : 'The workflow editor could not be loaded.';
	}

	onMount(() => {
		let active = true;
		let mounted: WorkflowEditorHandle | undefined;
		void (async () => {
			try {
				const { createWorkflowEditor } = await import('./rete-adapter');
				if (!active) return;
				mounted = await createWorkflowEditor({
					container,
					workflow,
					registry,
					readOnly,
					onChange: (change) => {
						if (active) {
							failure = '';
							onChange?.(change);
						}
					},
					onSelectionChange: (ids) => {
						if (!active) return;
						selected = ids;
						onSelectionChange?.(ids);
					},
					onViewportChange: (viewport) => {
						if (active) zoomPercent = viewport.zoomPercent;
					},
					onError: (error) => {
						if (active) {
							report(error);
							onError?.(error);
						}
					}
				});
				if (!active) {
					mounted.destroy();
					return;
				}
				controller = mounted;
				ready = true;
				onReady?.(mounted);
			} catch (error) {
				if (active) report(error);
			}
		})();
		return () => {
			active = false;
			disposed = true;
			mounted?.destroy();
			controller = null;
		};
	});

	$effect(() => {
		const next = workflow;
		if (!ready || !controller) return;
		const editor = controller;
		reconciliation = reconciliation
			.then(async () => {
				if (disposed) return;
				await editor.reconcile(next);
				if (disposed) return;
				failure = '';
				if (!hadNodes && next.nodes.length > 0) await editor.fitView();
				hadNodes = next.nodes.length > 0;
			})
			.catch(report);
	});

	function changeZoom(event: Event): void {
		if (event.currentTarget instanceof HTMLInputElement) {
			void controller?.setZoomPercent(event.currentTarget.valueAsNumber).catch(report);
		}
	}

	function keydown(event: KeyboardEvent): void {
		if (
			event.defaultPrevented ||
			readOnly ||
			!controller ||
			(event.key !== 'Delete' && event.key !== 'Backspace')
		)
			return;
		const target = event.target;
		if (
			target instanceof Element &&
			target.closest('input, textarea, select, button, [contenteditable="true"]')
		)
			return;
		if (!selected.length) return;
		event.preventDefault();
		event.stopPropagation();
		const editor = controller;
		void (async () => {
			for (const id of selected) await editor.removeNode(id);
		})().catch(report);
	}
</script>

<div
	class="workflow-editor"
	data-testid="workflow-editor"
	role="region"
	aria-label="Workflow canvas"
	aria-describedby={hintId}
>
	<!-- Rete mounts the interactive canvas here; this focus target owns its keyboard commands. -->
	<!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
	<div
		bind:this={container}
		class="workflow-area"
		data-workflow-area
		role="application"
		aria-label="Workflow editor"
		aria-describedby={hintId}
		onkeydown={keydown}
		tabindex="0"
	></div>
	{#if !ready && !failure}
		<div class="editor-message" role="status">
			<Spinner class="size-4" /> Loading workflow editor…
		</div>
	{/if}
	{#if failure}
		<div class="editor-error" role="alert">{failure}</div>
	{/if}
	{#if ready}
		<p class="pan-hint" aria-hidden="true">Drag the canvas to explore</p>
		<div class="viewport-controls" aria-label="Viewport controls">
			<Button
				variant="ghost"
				size="icon-sm"
				aria-label="Zoom out"
				onclick={() => void controller?.zoomOut().catch(report)}>−</Button
			>
			<input
				aria-label="Zoom"
				type="range"
				min="50"
				max="200"
				step="1"
				value={zoomPercent}
				oninput={changeZoom}
			/>
			<Button
				variant="ghost"
				size="icon-sm"
				aria-label="Zoom in"
				onclick={() => void controller?.zoomIn().catch(report)}>+</Button
			>
			<span class="zoom-value">{zoomPercent}%</span>
			<Button
				variant="ghost"
				size="sm"
				aria-label="Fit to view"
				onclick={() => void controller?.fitView().catch(report)}>Fit</Button
			>
		</div>
	{/if}
	<p id={hintId} class="sr-only">
		Drag empty space to pan. Scroll or pinch to zoom.
		{#if readOnly}
			This workflow preview is read-only.
		{:else}
			Select a node to inspect it. Use the palette and inspector for keyboard editing.
		{/if}
	</p>
</div>

<style>
	.workflow-editor {
		container-type: size;
		position: relative;
		width: 100%;
		height: 100%;
		min-height: 16rem;
		overflow: hidden;
		background: var(--surface-inset);
		color: var(--foreground);
	}
	.workflow-area:focus-visible {
		outline: 2px solid var(--ring);
		outline-offset: -2px;
	}
	.workflow-area {
		width: 100%;
		height: 100%;
		touch-action: none;
		background-image: radial-gradient(var(--border-subtle) 1px, transparent 1px);
		background-size: 20px 20px;
	}
	.viewport-controls {
		position: absolute;
		bottom: 0.75rem;
		left: 0.75rem;
		display: flex;
		align-items: center;
		gap: 0.25rem;
		padding: 0.2rem;
		border: 1px solid var(--border);
		border-radius: var(--radius-md);
		background: var(--card);
	}
	.viewport-controls input {
		width: 5rem;
		accent-color: var(--primary);
	}
	.viewport-controls input:focus-visible {
		outline: 2px solid var(--ring);
		outline-offset: 3px;
	}
	.zoom-value {
		min-width: 2.8rem;
		text-align: right;
		font-size: 0.6875rem;
		font-variant-numeric: tabular-nums;
	}
	.pan-hint {
		display: none;
		position: absolute;
		bottom: 3.75rem;
		left: 0.75rem;
		margin: 0;
		padding: 0.15rem 0.35rem;
		background: var(--surface-inset);
		color: var(--muted-foreground);
		font-size: 0.6875rem;
		pointer-events: none;
	}
	@container (max-width: 36rem) {
		.pan-hint {
			display: block;
		}
	}
	.editor-message {
		position: absolute;
		inset: 0;
		display: flex;
		gap: 0.5rem;
		align-items: center;
		justify-content: center;
		font-size: 0.75rem;
	}
	.editor-error {
		position: absolute;
		top: 0.75rem;
		left: 0.75rem;
		right: 0.75rem;
		padding: 0.65rem;
		border: 1px solid var(--destructive-border);
		border-radius: var(--radius-sm);
		background: var(--destructive-surface);
		color: var(--destructive);
		font-size: 0.75rem;
	}
</style>
