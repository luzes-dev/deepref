<script lang="ts">
	import type { WorkflowSocket } from './rete-types';

	let { data }: { data: WorkflowSocket } = $props();

	const direction = $derived(data.workflowDirection);
	const label = $derived(`${data.workflowLabel} · ${String(data.workflowDataType)}`);
</script>

<span
	class={`workflow-socket workflow-socket-${direction}`}
	data-workflow-port={data.workflowPortId}
	data-workflow-type={String(data.workflowDataType)}
	role="img"
	aria-label={label}
	title={label}
></span>

<style>
	.workflow-socket {
		display: inline-block;
		width: 0.75rem;
		height: 0.75rem;
		flex: 0 0 0.75rem;
		border: 2px solid var(--card);
		border-radius: 999px;
		background: var(--primary);
		box-shadow: 0 0 0 1px var(--border-strong);
		transition:
			transform 120ms ease,
			border-color 120ms ease;
		z-index: 2;
	}

	.workflow-socket:hover {
		border-color: var(--primary);
		transform: scale(1.2);
	}

	@media (prefers-reduced-motion: reduce) {
		.workflow-socket {
			transition: none;
		}
	}
</style>
