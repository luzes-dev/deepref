<script lang="ts">
	import { Root as BubbleRoot } from '../../primitives/bubble/index.js';
	import { cn, type WithElementRef } from '../../internal/utils.js';
	import type { HTMLAttributes } from 'svelte/elements';
	import WrenchIcon from '@lucide/svelte/icons/wrench';
	import LoaderCircleIcon from '@lucide/svelte/icons/loader-circle';
	import CircleCheckIcon from '@lucide/svelte/icons/circle-check';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import ChevronDownIcon from '@lucide/svelte/icons/chevron-down';

	let {
		ref = $bindable(null),
		class: className,
		toolName,
		arguments: rawArguments,
		args,
		status = 'completed',
		durationMs,
		duration,
		outputSummary,
		open = $bindable(false),
		bubble = true,
		...restProps
	}: WithElementRef<HTMLAttributes<HTMLDivElement>> & {
		toolName: string;
		arguments?: Record<string, unknown> | string;
		args?: Record<string, unknown> | string;
		status?: 'running' | 'completed' | 'failed';
		durationMs?: number;
		duration?: string;
		outputSummary?: string | Record<string, unknown>;
		open?: boolean;
		bubble?: boolean;
	} = $props();

	let resolvedArgs = $derived(args ?? rawArguments);

	let formattedDuration = $derived.by(() => {
		if (duration) return duration;
		if (durationMs !== undefined) {
			return durationMs >= 1000 ? `${(durationMs / 1000).toFixed(2)}s` : `${durationMs}ms`;
		}
		return null;
	});

	function toggleOpen() {
		open = !open;
	}
</script>

{#snippet cardInner()}
	<div class="flex w-full flex-col gap-2.5">
		<!-- Header / Toggle -->
		<button
			type="button"
			onclick={toggleOpen}
			aria-expanded={open}
			class="flex w-full items-center justify-between gap-3 text-left transition-opacity hover:opacity-90 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring rounded-xs"
		>
			<div class="flex min-w-0 items-center gap-2">
				<span class="flex size-6 shrink-0 items-center justify-center rounded-md bg-foreground/5 text-muted-foreground">
					<WrenchIcon class="size-3.5" />
				</span>
				<code
					data-slot="tool-name"
					class="truncate rounded bg-foreground/10 px-1.5 py-0.5 font-mono text-xs font-semibold text-foreground"
				>
					{toolName}
				</code>
			</div>

			<div class="flex shrink-0 items-center gap-2 text-xs">
				{#if status === 'running'}
					<span data-slot="tool-status" class="flex items-center gap-1 font-medium text-primary">
						<LoaderCircleIcon class="size-3.5 animate-spin" />
						<span>Running</span>
					</span>
				{:else if status === 'completed'}
					<span data-slot="tool-status" class="flex items-center gap-1 font-medium text-success">
						<CircleCheckIcon class="size-3.5" />
						<span>Completed</span>
					</span>
				{:else if status === 'failed'}
					<span data-slot="tool-status" class="flex items-center gap-1 font-medium text-destructive">
						<CircleAlertIcon class="size-3.5" />
						<span>Failed</span>
					</span>
				{/if}

				{#if formattedDuration}
					<span class="font-mono text-[11px] text-muted-foreground">{formattedDuration}</span>
				{/if}

				<ChevronDownIcon
					class={cn('size-4 text-muted-foreground transition-transform duration-150', open && 'rotate-180')}
				/>
			</div>
		</button>

		<!-- Collapsible details -->
		{#if open}
			<div class="mt-1 flex flex-col gap-2.5 border-t border-border/50 pt-2.5">
				{#if resolvedArgs}
					<div class="space-y-1">
						<span class="text-[10px] font-semibold tracking-wider text-muted-foreground uppercase">
							Arguments
						</span>
						{#if typeof resolvedArgs === 'object' && resolvedArgs !== null}
							<div
								data-slot="tool-arguments"
								class="max-h-48 overflow-y-auto rounded-md border border-border/50 bg-background/60 p-2 font-mono text-xs text-foreground"
							>
								{#each Object.entries(resolvedArgs) as [key, val]}
									<div class="flex items-start gap-1.5 py-0.5">
										<span class="shrink-0 text-muted-foreground select-none">{key}:</span>
										<span class="break-all text-foreground">
											{typeof val === 'object' ? JSON.stringify(val) : String(val)}
										</span>
									</div>
								{/each}
							</div>
						{:else}
							<pre
								data-slot="tool-arguments"
								class="max-h-48 overflow-x-auto rounded-md border border-border/50 bg-background/60 p-2 font-mono text-xs text-foreground"
							>{resolvedArgs}</pre>
						{/if}
					</div>
				{/if}

				{#if outputSummary}
					<div class="space-y-1">
						<span class="text-[10px] font-semibold tracking-wider text-muted-foreground uppercase">
							Output Summary
						</span>
						<div
							data-slot="tool-output"
							class="max-h-48 overflow-y-auto rounded-md border border-border/50 bg-background/60 p-2 text-xs font-mono text-foreground"
						>
							{#if typeof outputSummary === 'object' && outputSummary !== null}
								<pre class="overflow-x-auto whitespace-pre-wrap">{JSON.stringify(outputSummary, null, 2)}</pre>
							{:else}
								<p class="whitespace-pre-wrap break-words">{outputSummary}</p>
							{/if}
						</div>
					</div>
				{/if}
			</div>
		{/if}
	</div>
{/snippet}

{#if bubble}
	<BubbleRoot
		bind:ref
		variant="muted"
		data-slot="tool-call-card"
		class={cn('w-full border border-border/60', className)}
		{...restProps}
	>
		{@render cardInner()}
	</BubbleRoot>
{:else}
	<div
		bind:this={ref}
		data-slot="tool-call-card"
		class={cn('w-full rounded-2xl border border-border/60 bg-muted px-4 py-3 text-card-foreground shadow-2xs', className)}
		{...restProps}
	>
		{@render cardInner()}
	</div>
{/if}
