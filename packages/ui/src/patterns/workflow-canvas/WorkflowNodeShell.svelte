<script lang="ts">
	import type { Snippet } from "svelte";

	let {
		label,
		description,
		category,
		selected = false,
		children,
	}: {
		label: string;
		description?: string;
		category?: string;
		selected?: boolean;
		children?: Snippet;
	} = $props();
</script>

<div
	class={["node-shell", selected && "selected"]}
	data-slot="workflow-node-shell"
>
	<header>
		{#if category}<span class="category">{category}</span>{/if}
		<strong>{label}</strong>
	</header>
	{#if description}<p>{description}</p>{/if}
	{#if children}<div class="node-content">{@render children()}</div>{/if}
</div>

<style>
	.node-shell {
		width: 100%;
		min-width: 0;
		border: 1px solid var(--border-strong);
		border-radius: var(--radius-md);
		background: var(--card);
		color: var(--card-foreground);
	}
	.selected {
		border-color: var(--primary);
		box-shadow: 0 0 0 2px var(--selection);
	}
	header {
		display: flex;
		flex-direction: column;
		gap: 0.25rem;
		padding: 0.65rem 0.75rem;
		border-bottom: 1px solid var(--border-subtle);
	}
	.category {
		font-size: 0.625rem;
		line-height: 1.4;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: var(--muted-foreground);
	}
	strong {
		font-size: 0.8125rem;
		line-height: 1.4;
		font-weight: 600;
		overflow-wrap: anywhere;
	}
	p {
		padding: 0.65rem 0.75rem;
		margin: 0;
		font-size: 0.75rem;
		line-height: 1.5;
		color: var(--muted-foreground);
		overflow-wrap: anywhere;
	}
	.node-content {
		padding: 0.65rem 0.75rem;
	}
</style>
