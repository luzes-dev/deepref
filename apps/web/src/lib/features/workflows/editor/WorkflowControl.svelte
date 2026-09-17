<script lang="ts">
	import type { ClassicPreset } from 'rete';
	import { on } from 'svelte/events';
	import { WorkflowConfigControl } from './rete-types';

	let { data }: { data: ClassicPreset.Control } = $props();
	const config = $derived(data instanceof WorkflowConfigControl ? data : null);

	function stopPointerDown(element: HTMLInputElement): () => void {
		return on(element, 'pointerdown', (event) => event.stopPropagation());
	}

	function update(event: Event): void {
		if (!config) return;
		const target = event.currentTarget;
		if (target instanceof HTMLInputElement) {
			if (config.workflowConfigType === 'boolean') {
				config.setValue(target.checked);
			} else if (config.workflowConfigType === 'number') {
				if (Number.isFinite(target.valueAsNumber)) config.setValue(target.valueAsNumber);
			} else {
				config.setValue(target.value);
			}
		}
	}
</script>

<label class="workflow-control">
	{#if config}
		<span>{config.workflowConfigKey}</span>
		<input
			type={config.workflowConfigType === 'boolean' ? 'checkbox' : config.workflowConfigType}
			value={config.workflowConfigType === 'boolean' ? undefined : String(config.value)}
			checked={config.workflowConfigType === 'boolean' ? Boolean(config.value) : undefined}
			readonly={config.workflowReadonly && config.workflowConfigType !== 'boolean'}
			disabled={config.workflowReadonly}
			aria-label={config.workflowConfigKey}
			{@attach stopPointerDown}
			oninput={update}
		/>
	{/if}
</label>

<style>
	.workflow-control {
		display: flex;
		align-items: center;
		justify-content: space-between;
		gap: 0.5rem;
		font-size: 0.6875rem;
		line-height: 1.3;
		color: var(--muted-foreground);
	}

	.workflow-control span {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	input {
		width: 7.5rem;
		min-width: 0;
		border: 1px solid var(--input);
		border-radius: var(--radius-sm, 0.25rem);
		background: var(--background);
		color: var(--foreground);
		padding: 0.25rem 0.375rem;
		font: inherit;
	}

	input[type='checkbox'] {
		width: 1rem;
		height: 1rem;
		accent-color: var(--primary);
	}

	input:focus-visible {
		outline: 2px solid var(--ring);
		outline-offset: 2px;
	}
</style>
