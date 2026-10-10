<script lang="ts">
	import AlertTriangleIcon from '@lucide/svelte/icons/triangle-alert';
	import FlaskConicalIcon from '@lucide/svelte/icons/flask-conical';
	import Trash2Icon from '@lucide/svelte/icons/trash-2';
	import XIcon from '@lucide/svelte/icons/x';
	import { Button } from '@deepref/ui/button';
	import { Input } from '@deepref/ui/input';
	import { Label } from '@deepref/ui/label';
	import type {
		FieldDetailsDto,
		NodeRunDto,
		NodeTypeDto
	} from '#lib/api/generated/models/index.js';
	import ConnectionPanel from './ConnectionPanel.svelte';
	import DataView from './DataView.svelte';
	import ConfigFieldInput from './fields/ConfigFieldInput.svelte';
	import { iconForNode } from './icons';
	import { CATEGORY_LABEL, type Category, type FlowBlockData } from './model';

	let {
		projectId,
		workflowId,
		data,
		def,
		issues,
		runs = [],
		extractionFields,
		details = {},
		readOnly = false,
		onconfig,
		onrename,
		ondelete,
		onclose
	}: {
		projectId: string;
		workflowId: string;
		data: FlowBlockData;
		def: NodeTypeDto | undefined;
		issues: string[];
		runs?: NodeRunDto[];
		extractionFields: { id: string; label: string }[];
		/** What each text and rule field can read from the steps before it, by field key. */
		details?: Record<string, FieldDetailsDto>;
		readOnly?: boolean;
		onconfig: (key: string, value: unknown) => void;
		onrename: (label: string) => void;
		ondelete: () => void;
		onclose: () => void;
	} = $props();

	const Icon = $derived(iconForNode(data.typeId, def?.category));
	const STATUS_COPY: Record<string, string> = {
		completed: 'This step worked as expected.',
		failed: 'This step ran into a problem.',
		skipped: 'Nothing reached this step, so it did nothing.',
		running: 'This step is running now.',
		queued: 'This step is waiting its turn.',
		cancelled: 'This step was cancelled.'
	};
</script>

<aside
	class="flex h-full w-full flex-col border-l border-border bg-card"
	aria-label="Block settings"
	data-testid="inspector"
>
	<header class="flex items-start gap-2 border-b border-border p-3">
		<span
			class="flex size-8 shrink-0 items-center justify-center rounded-md bg-muted text-foreground"
		>
			<Icon class="size-4" />
		</span>
		<div class="min-w-0 flex-1">
			<p class="text-3xs tracking-caps text-muted-foreground uppercase">
				{CATEGORY_LABEL[(def?.category ?? 'action') as Category]}
			</p>
			<h2 class="text-sm leading-snug font-semibold">{def?.label ?? 'Unknown block'}</h2>
			{#if def}<p class="mt-0.5 text-xs text-muted-foreground">{def.description}</p>{/if}
		</div>
		<Button type="button" size="icon-sm" variant="ghost" aria-label="Close" onclick={onclose}>
			<XIcon />
		</Button>
	</header>

	<div class="flex min-h-0 flex-1 flex-col gap-4 overflow-y-auto p-3">
		{#if issues.length > 0}
			<ul
				class="flex flex-col gap-1 rounded-md border border-warning-border bg-warning-surface p-2 text-xs"
				data-testid="inspector-issues"
			>
				{#each issues as issue (issue)}
					<li class="flex items-start gap-1.5">
						<AlertTriangleIcon class="mt-px size-3.5 shrink-0 text-warning" />{issue}
					</li>
				{/each}
			</ul>
		{/if}

		<div class="flex flex-col gap-1.5">
			<Label for="block-name">Name on the canvas</Label>
			<Input
				id="block-name"
				disabled={readOnly}
				placeholder={def?.label ?? ''}
				value={data.customLabel ?? ''}
				oninput={(event) => onrename(event.currentTarget.value)}
			/>
		</div>

		{#if def}
			{#each def.config as field (field.key)}
				<ConfigFieldInput
					{field}
					value={data.config[field.key]}
					{extractionFields}
					details={details[field.key]}
					onchange={(next) => onconfig(field.key, next)}
				/>
			{/each}
			{#if data.typeId === 'trigger.webhook'}
				<ConnectionPanel {projectId} {workflowId} kind="webhook" />
			{:else if data.typeId === 'trigger.email'}
				<ConnectionPanel {projectId} {workflowId} kind="email" />
			{/if}
			{#if def.config.length === 0 && !data.typeId.startsWith('trigger.webhook') && data.typeId !== 'trigger.email'}
				<p class="text-sm text-muted-foreground">This block needs no settings.</p>
			{/if}
			{#if def.has_side_effects}
				<p
					class="flex items-start gap-1.5 rounded-md bg-muted p-2 text-xs text-muted-foreground"
				>
					<FlaskConicalIcon class="mt-px size-3.5 shrink-0" />
					Test runs never do this for real. They only show what would happen.
				</p>
			{/if}
		{/if}

		{#if runs.length > 0}
			<section
				class="flex flex-col gap-2 border-t border-border pt-3"
				data-testid="inspector-run"
			>
				<h3 class="text-xs font-semibold">Last test</h3>
				{#each runs as run (run.id)}
					<div class="flex flex-col gap-2 rounded-md border border-border p-2">
						<p class="text-xs">{STATUS_COPY[run.status] ?? run.status}</p>
						{#if run.note}<p class="text-xs text-muted-foreground">{run.note}</p>{/if}
						{#if run.error}
							<p class="rounded-md bg-destructive/10 p-2 text-xs text-destructive">
								{run.error}
							</p>
						{/if}
						<div class="flex flex-col gap-1">
							<p class="text-3xs tracking-caps text-muted-foreground uppercase">
								What came in
							</p>
							<DataView value={run.input} />
						</div>
						<div class="flex flex-col gap-1">
							<p class="text-3xs tracking-caps text-muted-foreground uppercase">
								What came out
							</p>
							<DataView value={run.output} />
						</div>
					</div>
				{/each}
			</section>
		{/if}
	</div>

	{#if !readOnly}
		<footer class="border-t border-border p-3">
			<Button type="button" size="sm" variant="outline" class="w-full" onclick={ondelete}>
				<Trash2Icon /> Delete this block
			</Button>
		</footer>
	{/if}
</aside>
