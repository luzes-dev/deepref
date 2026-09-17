<script lang="ts">
	import WorkflowNodeShell from "../../src/patterns/workflow-canvas/WorkflowNodeShell.svelte";
	import { Badge } from "../../src/primitives/badge";
	import { Input } from "../../src/primitives/input";
	let label = $state("Record normalization");
</script>

<main class="mx-auto max-w-5xl p-5 sm:p-8">
	<h1 class="mb-2 font-serif text-2xl">Workflow node presentation</h1>
	<p class="mb-7 max-w-2xl text-sm text-muted-foreground">
		Node surfaces use plain props and ordinary controls. The application
		editor owns ports, selection, movement, and connections.
	</p>
	<div class="grid items-start gap-5 sm:grid-cols-2 lg:grid-cols-3">
		<WorkflowNodeShell
			label="New record received"
			category="Trigger"
			description="Starts when a record arrives."
		>
			<Badge variant="outline">Ready</Badge>
		</WorkflowNodeShell>
		<WorkflowNodeShell
			{label}
			category="Action"
			selected
			description="Selected nodes retain the same surface hierarchy."
		>
			<label for="node-title" class="mb-1 block text-xs">Step label</label
			>
			<Input id="node-title" bind:value={label} />
		</WorkflowNodeShell>
		<WorkflowNodeShell
			label="Reviewer confirmation"
			category="Human review"
			description="Waits for a reviewer to confirm the result."
		>
			<Badge variant="warning">Review required</Badge>
		</WorkflowNodeShell>
		<WorkflowNodeShell
			label="A deliberately long processing step title that must remain readable in a constrained node"
			category="Transform"
			description="Supporting information wraps without widening the graph or clipping the title."
		/>
		<WorkflowNodeShell label="No optional content" />
		<WorkflowNodeShell label="Unavailable operation" category="Unsupported">
			<p class="text-xs text-destructive">
				This operation version is unavailable. The saved definition is
				retained.
			</p>
		</WorkflowNodeShell>
	</div>
</main>
