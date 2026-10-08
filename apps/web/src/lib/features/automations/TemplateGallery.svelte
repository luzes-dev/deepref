<script lang="ts">
	import { goto } from '$app/navigation';
	import { resolve } from '$app/paths';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import {
		createWorkflow,
		createWorkflowFromTemplate,
		createListAutomationTemplates,
		listWorkflows
	} from '$lib/api/generated/automations/automations';
	import * as Dialog from '@deepref/ui/dialog';
	import { Spinner } from '@deepref/ui/spinner';
	import { notifyError } from '$lib/features/notifications/toast';
	import { describeTrigger, uniqueName, type WfGraph } from './model';
	import { graphOf } from './api';

	let { projectId, open = $bindable(false) }: { projectId: string; open?: boolean } = $props();

	const templates = createListAutomationTemplates(() => ({ query: { enabled: open } }));
	let creating = $state<string | null>(null);

	async function takenNames(): Promise<string[]> {
		try {
			return (await listWorkflows(projectId)).data.map((item) => item.name);
		} catch {
			return [];
		}
	}

	async function open_(workflowId: string) {
		open = false;
		await goto(
			resolve('/projects/[projectId]/automations/[workflowId]', { projectId, workflowId })
		);
	}

	async function fromScratch() {
		creating = 'scratch';
		try {
			const response = await createWorkflow(projectId, {
				name: uniqueName('Untitled automation', await takenNames()),
				graph: { nodes: [], edges: [] } as never
			});
			await open_(response.data.id);
		} catch (error) {
			notifyError('Could not create the automation', error);
		} finally {
			creating = null;
		}
	}

	async function fromTemplate(templateId: string, name: string) {
		creating = templateId;
		try {
			const response = await createWorkflowFromTemplate(projectId, {
				template_id: templateId,
				name: uniqueName(name, await takenNames())
			});
			await open_(response.data.id);
		} catch (error) {
			notifyError('Could not create the automation', error);
		} finally {
			creating = null;
		}
	}
</script>

<Dialog.Root bind:open>
	<Dialog.Content
		class="max-h-[85vh] overflow-y-auto sm:max-w-3xl"
		data-testid="template-gallery"
	>
		<Dialog.Header>
			<Dialog.Title>New automation</Dialog.Title>
			<Dialog.Description>
				Pick a ready-made automation to adjust, or start with an empty canvas.
			</Dialog.Description>
		</Dialog.Header>
		<div class="grid gap-3 sm:grid-cols-2">
			<button
				type="button"
				class="flex min-h-28 flex-col items-start gap-1 rounded-lg border border-dashed border-border bg-card p-4 text-left transition-colors hover:bg-accent focus-visible:outline-2 focus-visible:outline-ring disabled:opacity-60"
				disabled={creating !== null}
				onclick={fromScratch}
				data-testid="start-from-scratch"
			>
				<span
					class="flex size-8 items-center justify-center rounded-md bg-primary/15 text-primary"
				>
					{#if creating === 'scratch'}<Spinner />{:else}<PlusIcon class="size-4" />{/if}
				</span>
				<span class="text-sm font-semibold">Start from scratch</span>
				<span class="text-xs text-muted-foreground"
					>An empty canvas. Add blocks as you like.</span
				>
			</button>
			{#if templates.isPending}
				<p class="flex items-center gap-2 text-sm text-muted-foreground">
					<Spinner /> Loading ideas…
				</p>
			{:else if templates.isError}
				<p class="text-sm text-destructive">Ready-made automations could not be loaded.</p>
			{/if}
			{#each templates.data?.data ?? [] as template (template.id)}
				<button
					type="button"
					class="flex min-h-28 flex-col items-start gap-1 rounded-lg border border-border bg-card p-4 text-left transition-colors hover:bg-accent focus-visible:outline-2 focus-visible:outline-ring disabled:opacity-60"
					disabled={creating !== null}
					onclick={() => fromTemplate(template.id, template.name)}
					data-testid="template-card"
				>
					<span class="text-sm font-semibold">
						{template.name}{#if creating === template.id}<Spinner
								class="ml-2 inline"
							/>{/if}
					</span>
					<span class="text-xs text-muted-foreground">{template.description}</span>
					<span class="mt-auto pt-2 text-2xs text-muted-foreground">
						{describeTrigger(graphOf(template) as WfGraph)}
					</span>
				</button>
			{/each}
		</div>
	</Dialog.Content>
</Dialog.Root>
