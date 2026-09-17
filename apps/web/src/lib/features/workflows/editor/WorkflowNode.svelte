<script lang="ts">
	import { Ref, type SvelteArea2D } from 'rete-svelte-plugin/5';
	import { WorkflowNodeShell } from '@deepref/ui/flow';

	import {
		WorkflowConfigControl,
		type WorkflowReteNode,
		type WorkflowSchemes
	} from './rete-types';

	type NodeEmit = (signal: SvelteArea2D<WorkflowSchemes>) => void;
	type ReteInput = NonNullable<WorkflowReteNode['inputs'][string]>;
	type ReteOutput = NonNullable<WorkflowReteNode['outputs'][string]>;

	let { data, emit }: { data: WorkflowReteNode; emit: NodeEmit } = $props();

	function definedEntries<T>(record: Record<string, T | undefined>): [string, T][] {
		return Object.entries(record).flatMap(([key, value]) => (value ? [[key, value]] : []));
	}

	const inputs = $derived(definedEntries(data.inputs));
	const outputs = $derived(definedEntries(data.outputs));
	const controls = $derived(
		definedEntries(data.controls).filter(
			(entry): entry is [string, WorkflowConfigControl] =>
				entry[1] instanceof WorkflowConfigControl
		)
	);
	const title = $derived(
		data.workflowNode.label ?? readMetadataText('label') ?? data.workflowTitle
	);
	const description = $derived(
		readMetadataText('description') ??
			(data.workflowUnsupported
				? `No renderer definition is registered for ${String(data.workflowKind)}.`
				: data.workflowDescription)
	);
	const category = $derived(
		data.workflowUnsupported ? 'Unsupported node' : data.workflowCategory
	);

	function readMetadataText(key: string): string | undefined {
		const value = data.workflowNode.metadata?.[key];
		return typeof value === 'string' ? value : undefined;
	}

	function renderSocket(
		element: HTMLElement,
		side: 'input' | 'output',
		key: string,
		socket: ReteInput['socket'] | ReteOutput['socket']
	): void {
		emit({
			type: 'render',
			data: {
				type: 'socket',
				side,
				key,
				nodeId: data.id,
				element,
				payload: socket
			}
		});
	}

	function unmount(element: HTMLElement): void {
		emit({ type: 'unmount', data: { element } });
	}
</script>

<div class="workflow-rete-node" data-workflow-node={String(data.workflowNodeId)}>
	<WorkflowNodeShell label={title} {description} {category} selected={data.selected === true}>
		<div class="workflow-node-body">
			{#if inputs.length > 0}
				<div class="workflow-port-group workflow-port-inputs" aria-label="Inputs">
					{#each inputs as [key, input] (key)}
						<div
							class="workflow-port-row workflow-port-row-input"
							title={input.socket.name}
						>
							<Ref
								class="workflow-socket-ref"
								init={(element: HTMLElement) =>
									renderSocket(element, 'input', key, input.socket)}
								{unmount}
							/>
							<span>{input.label ?? key}</span>
						</div>
					{/each}
				</div>
			{/if}

			{#if controls.length > 0}
				<div class="workflow-config" aria-label="Node configuration">
					{#each controls as [key, control] (key)}
						<Ref
							class="workflow-control-ref"
							data-workflow-control={key}
							init={(element: HTMLElement) =>
								emit({
									type: 'render',
									data: { type: 'control', element, payload: control }
								})}
							{unmount}
						/>
					{/each}
				</div>
			{/if}

			{#if outputs.length > 0}
				<div class="workflow-port-group workflow-port-outputs" aria-label="Outputs">
					{#each outputs as [key, output] (key)}
						<div
							class="workflow-port-row workflow-port-row-output"
							title={output.socket.name}
						>
							<span>{output.label ?? key}</span>
							<Ref
								class="workflow-socket-ref"
								init={(element: HTMLElement) =>
									renderSocket(element, 'output', key, output.socket)}
								{unmount}
							/>
						</div>
					{/each}
				</div>
			{/if}
		</div>
	</WorkflowNodeShell>
</div>

<style>
	.workflow-rete-node {
		width: 17rem;
		max-width: 20rem;
		font-family: var(--font-sans, 'IBM Plex Sans', sans-serif);
		user-select: none;
	}

	.workflow-node-body {
		display: grid;
		gap: 0.5rem;
	}

	.workflow-port-group {
		display: grid;
		gap: 0.35rem;
		font-size: 0.6875rem;
		line-height: 1.25;
		color: var(--foreground);
	}

	.workflow-port-group::before {
		font-size: 0.625rem;
		font-weight: 600;
		letter-spacing: 0.06em;
		text-transform: uppercase;
		color: var(--muted-foreground);
	}

	.workflow-port-inputs::before {
		content: 'Inputs';
	}

	.workflow-port-outputs::before {
		content: 'Outputs';
	}

	.workflow-port-row {
		display: flex;
		align-items: center;
		gap: 0.4rem;
		min-height: 1.25rem;
	}

	.workflow-port-row-input {
		justify-content: flex-start;
	}

	.workflow-port-row-output {
		justify-content: flex-end;
	}

	.workflow-port-row span {
		min-width: 0;
		overflow: hidden;
		text-overflow: ellipsis;
		white-space: nowrap;
	}

	.workflow-config {
		display: grid;
		gap: 0.4rem;
		padding-top: 0.1rem;
		border-top: 1px solid var(--border-subtle);
	}

	:global(.workflow-socket-ref) {
		display: inline-flex;
		align-items: center;
		justify-content: center;
		width: 0.75rem;
		height: 0.75rem;
		flex: 0 0 0.75rem;
	}

	:global(.workflow-control-ref) {
		display: block;
		min-width: 0;
	}
</style>
