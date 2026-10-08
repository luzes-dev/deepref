<script lang="ts">
	import type { Component } from 'svelte';

	let {
		decision,
		label,
		shortcut,
		icon: Icon,
		selected = false,
		disabled = false,
		onclick
	}: {
		decision: 'include' | 'exclude' | 'maybe';
		label: string;
		shortcut: string;
		icon: Component<{ class?: string; 'aria-hidden'?: 'true' }>;
		selected?: boolean;
		disabled?: boolean;
		onclick: () => void;
	} = $props();
</script>

<!-- A toggle whose pressed state is the report's recorded decision, so no separate status badge is needed. -->
<button
	type="button"
	class="decision"
	data-decision={decision}
	aria-pressed={selected}
	{disabled}
	{onclick}
>
	<span class="flex items-center gap-2"><Icon class="size-4" aria-hidden="true" />{label}</span>
	<kbd aria-hidden="true">{shortcut}</kbd>
</button>

<style>
	.decision {
		display: inline-flex;
		min-height: 2.5rem;
		flex: 1 1 0;
		align-items: center;
		justify-content: center;
		gap: 0.75rem;
		border: 1px solid var(--border);
		border-radius: var(--radius);
		background: var(--background);
		padding: 0 0.5rem;
		font-size: 0.875rem;
		font-weight: 500;
		color: var(--foreground);
		transition:
			background-color 120ms,
			border-color 120ms,
			color 120ms;
	}
	.decision:hover:not(:disabled) {
		background: var(--muted);
	}
	.decision:focus-visible {
		outline: 2px solid var(--ring);
		outline-offset: 2px;
	}
	.decision:disabled {
		cursor: not-allowed;
		opacity: 0.5;
	}
	.decision[aria-pressed='true'] {
		color: var(--background);
	}
	.decision[aria-pressed='true'][data-decision='include'] {
		border-color: var(--success);
		background: var(--success);
	}
	.decision[aria-pressed='true'][data-decision='exclude'] {
		border-color: var(--destructive);
		background: var(--destructive);
	}
	.decision[aria-pressed='true'][data-decision='maybe'] {
		border-color: var(--warning);
		background: var(--warning);
	}
	kbd {
		display: none;
		min-width: 1.25rem;
		justify-content: center;
		border-radius: 0.25rem;
		background: color-mix(in oklab, currentColor 12%, transparent);
		padding: 0 0.3rem;
		font-family: var(--font-sans);
		font-size: 0.6875rem;
		font-weight: 600;
	}
	@media (min-width: 40rem) {
		.decision {
			max-width: 11rem;
			justify-content: space-between;
			padding: 0 0.75rem;
		}
		kbd {
			display: inline-flex;
		}
	}
</style>
