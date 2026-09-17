<script lang="ts">
	import type { Component } from 'svelte';
	import GripVerticalIcon from '@lucide/svelte/icons/grip-vertical';
	import SearchIcon from '@lucide/svelte/icons/search';
	import CommandIcon from '@lucide/svelte/icons/command';
	import PanelRightCloseIcon from '@lucide/svelte/icons/panel-right-close';
	import PanelRightOpenIcon from '@lucide/svelte/icons/panel-right-open';
	import PlusIcon from '@lucide/svelte/icons/plus';

	export interface GalleryNodeItem {
		id: string;
		name: string;
		label?: string;
		description?: string;
		category: string;
		icon: Component<{ class?: string; 'data-icon'?: string }>;
		iconColor?: string;
		iconBg?: string;
		badge?: string;
		testId?: string;
		type?: string;
		defaultData?: Record<string, unknown>;
	}

	export interface GalleryCategory {
		id: string;
		title: string;
		items: GalleryNodeItem[];
	}

	let {
		title = 'Node Gallery',
		subtitle = 'Drag or click to mount onto flow',
		categories = [],
		selectedItemId = null,
		onSelect,
		onAddNode,
		collapsible = true
	}: {
		title?: string;
		subtitle?: string;
		categories: GalleryCategory[];
		selectedItemId?: string | null;
		onSelect?: (item: GalleryNodeItem) => void;
		onAddNode?: (item: GalleryNodeItem) => void;
		collapsible?: boolean;
	} = $props();

	let searchQuery = $state('');
	let isCollapsed = $state(false);

	const filteredCategories = $derived.by(() => {
		const query = searchQuery.trim().toLowerCase();
		if (!query) return categories;

		return categories
			.map((cat) => ({
				...cat,
				items: cat.items.filter(
					(item) =>
						item.name.toLowerCase().includes(query) ||
						(item.label && item.label.toLowerCase().includes(query)) ||
						(item.description && item.description.toLowerCase().includes(query))
				)
			}))
			.filter((cat) => cat.items.length > 0);
	});

	function handleDragStart(event: DragEvent, item: GalleryNodeItem): void {
		if (!event.dataTransfer) return;
		event.dataTransfer.setData(
			'application/deepref-node',
			JSON.stringify({
				type: item.type ?? 'action',
				data: item.defaultData ?? { label: item.label ?? item.name }
			})
		);
		event.dataTransfer.effectAllowed = 'move';
	}
</script>

<aside
	class={[
		'flex h-full min-w-0 flex-col rounded-lg border border-border/80 bg-card text-foreground shadow-none transition-all duration-300 select-none',
		isCollapsed ? 'w-12' : 'w-full'
	]}
	aria-label={title}
>
	<!-- Gallery Header -->
	<div
		class="flex items-center justify-between rounded-t-lg border-b border-border bg-card px-3.5 py-3"
	>
		{#if !isCollapsed}
			<div class="flex min-w-0 items-center gap-2.5">
				<div
					class="flex size-7 shrink-0 items-center justify-center rounded-lg bg-primary text-primary-foreground "
				>
					<CommandIcon class="size-4" />
				</div>
				<div class="flex min-w-0 flex-col">
					<h3 class="truncate text-sm font-semibold tracking-tight text-foreground">
						{title}
					</h3>
					{#if subtitle}
						<span class="truncate text-[10px] text-muted-foreground">{subtitle}</span>
					{/if}
				</div>
			</div>
		{/if}

		{#if collapsible}
			<button
				type="button"
				class="flex size-7 shrink-0 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-interactive-hover hover:text-foreground"
				onclick={() => (isCollapsed = !isCollapsed)}
				title={isCollapsed ? 'Expand gallery' : 'Collapse gallery'}
			>
				{#if isCollapsed}
					<PanelRightOpenIcon class="size-4" />
				{:else}
					<PanelRightCloseIcon class="size-4" />
				{/if}
			</button>
		{/if}
	</div>

	{#if !isCollapsed}
		<!-- Search Filter Bar -->
		<div class="border-b border-border p-3">
			<div class="relative">
				<SearchIcon
					class="absolute top-1/2 left-2.5 size-3.5 -translate-y-1/2 text-muted-foreground"
				/>
				<input
					type="search"
					placeholder="Search nodes & tools..."
					bind:value={searchQuery}
					class="h-8 w-full rounded-lg border border-border bg-card pr-3 pl-8 text-xs text-foreground transition-all placeholder:text-muted-foreground/70 focus:border-primary focus:ring-1 focus:ring-primary/30 focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring"
				/>
			</div>
		</div>

		<!-- Categorized List of Node Items -->
		<div class="custom-scrollbar min-h-0 flex-1 space-y-4 overflow-y-auto p-3">
			{#if filteredCategories.length === 0}
				<div class="py-8 text-center text-xs text-muted-foreground">
					No matching nodes found.
				</div>
			{:else}
				{#each filteredCategories as category (category.id)}
					<div class="space-y-1.5">
						<div class="flex items-center justify-between px-1 py-0.5">
							<span
								class="text-[11px] font-semibold tracking-wider text-muted-foreground/80 uppercase"
							>
								{category.title}
							</span>
							<span class="font-mono text-[10px] text-muted-foreground/50">
								{category.items.length}
							</span>
						</div>

						<div class="grid gap-1.5">
							{#each category.items as item (item.id)}
								{@const isSelected = selectedItemId === item.id}
								<div
									role="button"
									tabindex="0"
									draggable="true"
									ondragstart={(e) => handleDragStart(e, item)}
									onclick={() => onSelect?.(item)}
									onkeydown={(e) => {
										if (e.key === 'Enter' || e.key === ' ') {
											e.preventDefault();
											onSelect?.(item);
										}
									}}
									data-testid={item.testId}
									class={[
										'group relative flex cursor-pointer items-center justify-between gap-2.5 rounded-lg border px-3 py-2.5 text-left transition-all duration-150',
										isSelected
											? 'border-primary/80 bg-primary/40 text-foreground shadow-md '
											: 'border-border bg-card text-foreground hover:border-primary/50 hover:bg-interactive-hover hover:text-foreground'
									]}
								>
									<!-- Left: Icon & Label -->
									<div class="flex min-w-0 items-center gap-2.5">
										<div
											class={[
												'flex size-6 shrink-0 items-center justify-center rounded-md text-xs transition-colors',
												item.iconBg ?? 'bg-primary/15',
												item.iconColor ?? 'text-primary'
											]}
										>
											<item.icon class="size-3.5" />
										</div>
										<div class="flex min-w-0 flex-col">
											<span
												class="truncate text-xs font-medium tracking-tight"
											>
												{item.label ?? item.name}
											</span>
											{#if item.description}
												<span
													class="truncate text-[10px] text-muted-foreground"
												>
													{item.description}
												</span>
											{/if}
										</div>
									</div>

									<!-- Right: Micro Add button & Grip Handle -->
									<div class="flex shrink-0 items-center gap-1.5">
										{#if onAddNode}
											<button
												type="button"
												class="flex size-6 items-center justify-center rounded bg-primary/80 text-foreground opacity-0 transition-opacity group-hover:opacity-100 hover:bg-primary"
												onclick={(e) => {
													e.stopPropagation();
													onAddNode(item);
												}}
												title="Add to canvas"
											>
												<PlusIcon class="size-3" />
											</button>
										{/if}
										<div
											class="cursor-grab text-muted-foreground/40 transition-colors group-hover:text-primary/80 active:cursor-grabbing"
											title="Drag to canvas"
										>
											<GripVerticalIcon class="size-4" />
										</div>
									</div>
								</div>
							{/each}
						</div>
					</div>
				{/each}
			{/if}
		</div>
	{/if}
</aside>

<style>
	.custom-scrollbar::-webkit-scrollbar {
		width: 4px;
	}
	.custom-scrollbar::-webkit-scrollbar-track {
		background: transparent;
	}
	.custom-scrollbar::-webkit-scrollbar-thumb {
		background: var(--border);
		border-radius: 9999px;
	}
	.custom-scrollbar::-webkit-scrollbar-thumb:hover {
		background: var(--border-strong);
	}
</style>
