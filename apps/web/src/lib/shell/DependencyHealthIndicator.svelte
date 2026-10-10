<script lang="ts">
	import * as Popover from '@deepref/ui/popover';
	import { Button } from '@deepref/ui/button';
	import { Spinner } from '@deepref/ui/spinner';
	import { cn } from '@deepref/ui/utils';
	import { createGetDependencyStatus } from '#lib/api/generated/health/health.js';
	import { summarizeDependencyHealth } from './dependency-health';

	// Scoped to the project in view, so one project's failures do not colour
	// another project's pages. Without a project the probe covers the workspace.
	let { projectId = null }: { projectId?: string | null } = $props();

	const dependenciesQuery = createGetDependencyStatus(
		() => ({ project_id: projectId ?? undefined }),
		() => ({
			query: {
				refetchInterval: 10_000,
				refetchIntervalInBackground: false,
				refetchOnWindowFocus: 'always',
				staleTime: 5_000
			}
		})
	);

	const summary = $derived(
		summarizeDependencyHealth(dependenciesQuery.data?.data, dependenciesQuery.error, {
			scope: projectId ? 'project' : 'workspace'
		})
	);
	const label = $derived(summary.level === 'unavailable' ? 'Service issue' : 'Degraded');
</script>

{#if summary.level !== 'ok'}
	<Popover.Root>
		<Popover.Trigger>
			{#snippet child({ props })}
				<button
					{...props}
					type="button"
					class="inline-flex h-8 items-center gap-1.5 rounded-md px-2 text-xs font-medium text-muted-foreground transition-colors hover:bg-muted/70 hover:text-foreground focus-visible:ring-1 focus-visible:ring-ring focus-visible:outline-hidden"
					aria-label={`System status: ${label.toLowerCase()}`}
					data-testid="dependency-health-indicator"
					data-level={summary.level}
				>
					<span
						class={cn(
							'size-2 shrink-0 rounded-full',
							summary.level === 'unavailable' ? 'bg-destructive' : 'bg-warning'
						)}
						aria-hidden="true"
					></span>
					<span class="hidden sm:inline">{label}</span>
				</button>
			{/snippet}
		</Popover.Trigger>
		<Popover.Content align="end" class="w-80 gap-3" data-testid="dependency-health-panel">
			<p class="text-sm font-semibold">
				{summary.level === 'unavailable'
					? 'Some services are unavailable'
					: 'Some features are degraded'}
			</p>
			<ul class="grid gap-2">
				{#each summary.issues as issue (issue.name)}
					<li class="text-sm leading-5" data-testid="dependency-health-issue">
						<span class="font-medium">{issue.label}</span>
						<span class="text-muted-foreground"> — {issue.message}</span>
					</li>
				{/each}
			</ul>
			<Button
				variant="outline"
				size="sm"
				class="justify-self-start"
				disabled={dependenciesQuery.isFetching}
				onclick={() => void dependenciesQuery.refetch()}
			>
				{#if dependenciesQuery.isFetching}<Spinner data-icon="inline-start" />{/if}
				Refresh
			</Button>
		</Popover.Content>
	</Popover.Root>
{/if}
