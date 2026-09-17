<script lang="ts">
	let {
		id,
		isPseudo = false,
		path = ''
	}: {
		/**
		 * ConnectionWrapper spreads the Rete connection onto the custom
		 * component. The adapter id is therefore the stable serialized
		 * connection identity available here; the non-enumerable adapter
		 * metadata intentionally stays out of renderer props.
		 */
		id?: string;
		isPseudo?: boolean;
		path?: string;
	} = $props();

	const className = $derived(
		isPseudo ? 'workflow-connection workflow-connection-pseudo' : 'workflow-connection'
	);
</script>

<svg class={className} data-workflow-connection={id ?? ''} aria-hidden="true">
	<path d={path} />
</svg>

<style>
	.workflow-connection {
		position: absolute;
		overflow: visible !important;
		width: 9999px;
		height: 9999px;
		pointer-events: none;
	}

	.workflow-connection path {
		fill: none;
		stroke: var(--primary);
		stroke-width: 2px;
		stroke-linecap: round;
		pointer-events: stroke;
	}

	.workflow-connection-pseudo path {
		stroke: var(--muted-foreground);
		stroke-dasharray: 5 5;
	}
</style>
