<script lang="ts">
	import type { Snippet } from 'svelte';
	import { cn } from '$lib/utils';
	import * as DropdownMenu from '@deepref/ui/dropdown-menu';
	import ChevronsUpDownIcon from '@lucide/svelte/icons/chevrons-up-down';
	import CheckIcon from '@lucide/svelte/icons/check';
	import PlusIcon from '@lucide/svelte/icons/plus';
	import NotificationBell from './NotificationBell.svelte';

	export type TopNavBarProject = {
		id: string;
		name: string;
	};

	let {
		title = 'Overview',
		titleSnippet,
		scopeLabel,
		projects = [],
		selectedProjectId = null,
		onSelectProject,
		onCreateProject,
		agentLabel = 'Agent',
		agentHref,
		onAgentClick,
		agentSnippet,
		leftSnippet,
		rightSnippet,
		class: className = ''
	}: {
		title?: string;
		titleSnippet?: Snippet;
		scopeLabel?: string;
		projects?: TopNavBarProject[];
		selectedProjectId?: string | null;
		onSelectProject?: (projectId: string | null) => void;
		onCreateProject?: () => void;
		agentLabel?: string;
		agentHref?: string;
		onAgentClick?: () => void;
		agentSnippet?: Snippet;
		leftSnippet?: Snippet;
		rightSnippet?: Snippet;
		class?: string;
	} = $props();

	const currentLabel = $derived.by(() => {
		if (scopeLabel) return scopeLabel;
		if (selectedProjectId && projects.length > 0) {
			const matched = projects.find((p) => p.id === selectedProjectId);
			if (matched) return matched.name;
		}
		return 'All Projects';
	});
</script>

<header
	class={cn(
		'relative flex h-12 w-full items-center justify-between border-b border-border/50 bg-background px-4 text-foreground sm:px-6',
		className
	)}
	data-top-navbar
>
	<!-- Left: Scope / Project Selector -->
	<div class="flex items-center gap-2">
		{#if leftSnippet}
			{@render leftSnippet()}
		{:else if projects.length > 0 || onSelectProject || onCreateProject}
			<DropdownMenu.Root>
				<DropdownMenu.Trigger>
					{#snippet child({ props })}
						<button
							{...props}
							type="button"
							class="inline-flex items-center gap-1.5 rounded-md px-2 py-1 text-sm font-medium text-foreground transition-colors hover:bg-muted/70 focus-visible:ring-1 focus-visible:ring-ring focus-visible:outline-hidden"
							aria-label="Switch project scope"
						>
							<span class="max-w-[200px] truncate">{currentLabel}</span>
							<ChevronsUpDownIcon
								class="size-3.5 shrink-0 text-muted-foreground"
								aria-hidden="true"
							/>
						</button>
					{/snippet}
				</DropdownMenu.Trigger>
				<DropdownMenu.Content align="start" class="w-56">
					<DropdownMenu.Group>
						<DropdownMenu.Item
							onSelect={() => onSelectProject?.(null)}
							class="flex items-center justify-between"
						>
							<span>All Projects</span>
							{#if selectedProjectId === null}
								<CheckIcon class="size-4 text-primary" aria-hidden="true" />
							{/if}
						</DropdownMenu.Item>
					</DropdownMenu.Group>

					{#if projects.length > 0}
						<DropdownMenu.Separator />
						<DropdownMenu.Group class="max-h-60 overflow-y-auto">
							{#each projects as proj (proj.id)}
								<DropdownMenu.Item
									onSelect={() => onSelectProject?.(proj.id)}
									class="flex items-center justify-between"
								>
									<span class="truncate">{proj.name}</span>
									{#if selectedProjectId === proj.id}
										<CheckIcon class="size-4 text-primary" aria-hidden="true" />
									{/if}
								</DropdownMenu.Item>
							{/each}
						</DropdownMenu.Group>
					{/if}

					{#if onCreateProject}
						<DropdownMenu.Separator />
						<DropdownMenu.Group>
							<DropdownMenu.Item onSelect={() => onCreateProject()}>
								<PlusIcon class="size-4" aria-hidden="true" />
								<span>Create project</span>
							</DropdownMenu.Item>
						</DropdownMenu.Group>
					{/if}
				</DropdownMenu.Content>
			</DropdownMenu.Root>
		{:else}
			<button
				type="button"
				class="inline-flex items-center gap-1.5 rounded-md px-2 py-1 text-sm font-medium text-foreground transition-colors hover:bg-muted/70 focus-visible:ring-1 focus-visible:ring-ring focus-visible:outline-hidden"
				aria-label="Project scope"
			>
				<span class="max-w-[200px] truncate">{currentLabel}</span>
				<ChevronsUpDownIcon
					class="size-3.5 shrink-0 text-muted-foreground"
					aria-hidden="true"
				/>
			</button>
		{/if}
	</div>

	<!-- Center: Title -->
	<div
		class="pointer-events-none absolute top-1/2 left-1/2 -translate-x-1/2 -translate-y-1/2 text-center"
	>
		{#if titleSnippet}
			<div class="pointer-events-auto">
				{@render titleSnippet()}
			</div>
		{:else}
			<h1 class="text-sm font-medium tracking-tight text-foreground select-none">
				{title}
			</h1>
		{/if}
	</div>

	<!-- Right: Notifications + Agent Action -->
	<div class="flex items-center gap-2">
		<NotificationBell />
		{#if rightSnippet}
			{@render rightSnippet()}
		{:else if agentSnippet}
			{@render agentSnippet()}
		{:else if agentHref}
			<a
				href={agentHref}
				class="inline-flex items-center gap-1.5 rounded-md px-2.5 py-1 text-sm font-medium text-muted-foreground transition-colors hover:bg-muted/70 hover:text-foreground focus-visible:ring-1 focus-visible:ring-ring focus-visible:outline-hidden"
			>
				<svg
					class="size-4 shrink-0 text-current"
					viewBox="0 0 16 16"
					fill="none"
					xmlns="http://www.w3.org/2000/svg"
					aria-hidden="true"
				>
					<circle cx="8" cy="3" r="1.25" fill="currentColor" />
					<circle cx="3.5" cy="11.5" r="1.25" fill="currentColor" />
					<circle cx="12.5" cy="11.5" r="1.25" fill="currentColor" />
					<path
						d="M8 4.25L4.25 10.75M8 4.25L11.75 10.75M4.75 11.5H11.25"
						stroke="currentColor"
						stroke-width="1"
						stroke-linecap="round"
					/>
				</svg>
				<span>{agentLabel}</span>
			</a>
		{:else}
			<button
				type="button"
				onclick={() => onAgentClick?.()}
				class="inline-flex items-center gap-1.5 rounded-md px-2.5 py-1 text-sm font-medium text-muted-foreground transition-colors hover:bg-muted/70 hover:text-foreground focus-visible:ring-1 focus-visible:ring-ring focus-visible:outline-hidden"
			>
				<svg
					class="size-4 shrink-0 text-current"
					viewBox="0 0 16 16"
					fill="none"
					xmlns="http://www.w3.org/2000/svg"
					aria-hidden="true"
				>
					<circle cx="8" cy="3" r="1.25" fill="currentColor" />
					<circle cx="3.5" cy="11.5" r="1.25" fill="currentColor" />
					<circle cx="12.5" cy="11.5" r="1.25" fill="currentColor" />
					<path
						d="M8 4.25L4.25 10.75M8 4.25L11.75 10.75M4.75 11.5H11.25"
						stroke="currentColor"
						stroke-width="1"
						stroke-linecap="round"
					/>
				</svg>
				<span>{agentLabel}</span>
			</button>
		{/if}
	</div>
</header>
