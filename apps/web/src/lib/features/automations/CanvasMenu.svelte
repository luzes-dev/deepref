<script lang="ts" module>
	import type { Component } from 'svelte';

	export type MenuEntry =
		| {
				kind: 'item';
				id: string;
				label: string;
				icon?: Component<{ class?: string }>;
				shortcut?: string;
				danger?: boolean;
				disabled?: boolean;
				onselect: () => void;
		  }
		| { kind: 'add'; label: string; icon?: Component<{ class?: string }> }
		| { kind: 'separator' };
</script>

<script lang="ts">
	import { tick } from 'svelte';
	import ChevronLeftIcon from '@lucide/svelte/icons/chevron-left';
	import ChevronRightIcon from '@lucide/svelte/icons/chevron-right';
	import SearchIcon from '@lucide/svelte/icons/search';
	import XIcon from '@lucide/svelte/icons/x';
	import type { NodeTypeDto } from '#lib/api/generated/models/index.js';
	import { iconForNode } from './icons';
	import {
		BLOCK_ALIASES,
		CATEGORY_LABEL,
		CATEGORY_ORDER,
		portsCompatible,
		toneOf
	} from './model';

	let {
		x,
		y,
		entries = [],
		startInAdd = false,
		catalog,
		accepts = null,
		hasTrigger = false,
		presentation = 'popover',
		onpick,
		onclose
	}: {
		x: number;
		y: number;
		entries?: MenuEntry[];
		startInAdd?: boolean;
		catalog: NodeTypeDto[];
		/** Port type of the output being extended; limits the list to blocks that can take it. */
		accepts?: string | null;
		hasTrigger?: boolean;
		/** A popover at (x, y) on a computer; a sheet along the bottom on a phone. */
		presentation?: 'popover' | 'sheet';
		onpick: (def: NodeTypeDto) => void;
		onclose: () => void;
	} = $props();
	const sheet = $derived(presentation === 'sheet');

	let view = $state<'root' | 'add'>('root');
	let search = $state('');
	let active = $state(0);
	let container = $state<HTMLDivElement | null>(null);
	let searchInput = $state<HTMLInputElement | null>(null);
	let position = $state({ left: 0, top: 0 });
	let initialised = false;

	$effect(() => {
		if (initialised) return;
		initialised = true;
		view = startInAdd || entries.length === 0 ? 'add' : 'root';
	});

	const choices = $derived.by(() => {
		const query = search.trim().toLowerCase();
		return catalog.filter((def) => {
			if (def.category === 'trigger' && (hasTrigger || accepts)) return false;
			if (accepts && !def.inputs.some((input) => portsCompatible(accepts, input.type)))
				return false;
			if (!query) return true;
			return (
				def.label.toLowerCase().includes(query) ||
				def.description.toLowerCase().includes(query) ||
				(BLOCK_ALIASES[def.id] ?? '').includes(query) ||
				CATEGORY_LABEL[def.category as keyof typeof CATEGORY_LABEL]
					?.toLowerCase()
					.includes(query)
			);
		});
	});
	const groups = $derived(
		CATEGORY_ORDER.map((category) => ({
			category,
			label: CATEGORY_LABEL[category],
			items: choices.filter((def) => def.category === category)
		})).filter((group) => group.items.length > 0)
	);
	const flat = $derived(groups.flatMap((group) => group.items));

	$effect(() => {
		void view;
		void search;
		void choices.length;
		// Keep the menu on screen after its content changes size.
		void tick().then(() => {
			if (!container || sheet) return;
			const box = container.getBoundingClientRect();
			position = {
				left: Math.max(8, Math.min(x, window.innerWidth - box.width - 8)),
				top: Math.max(8, Math.min(y, window.innerHeight - box.height - 8))
			};
		});
	});

	$effect(() => {
		void view;
		void tick().then(() => {
			if (view === 'add') searchInput?.focus();
			else
				container?.querySelector<HTMLElement>('[role="menuitem"]:not([disabled])')?.focus();
		});
	});

	$effect(() => {
		void search;
		active = 0;
	});

	$effect(() => {
		const el = container?.querySelector<HTMLElement>(`[data-index="${active}"]`);
		el?.scrollIntoView({ block: 'nearest' });
	});

	function onWindowPointer(event: PointerEvent) {
		if (container && !container.contains(event.target as globalThis.Node)) onclose();
	}

	function onRootKey(event: KeyboardEvent) {
		const buttons = [
			...(container?.querySelectorAll<HTMLElement>('[role="menuitem"]:not([disabled])') ?? [])
		];
		const index = buttons.indexOf(document.activeElement as HTMLElement);
		if (event.key === 'ArrowDown') buttons[(index + 1) % buttons.length]?.focus();
		else if (event.key === 'ArrowUp')
			buttons[(index - 1 + buttons.length) % buttons.length]?.focus();
		else if (event.key === 'Home') buttons[0]?.focus();
		else if (event.key === 'End') buttons[buttons.length - 1]?.focus();
		else if (
			event.key === 'ArrowRight' &&
			(document.activeElement as HTMLElement)?.dataset.add
		) {
			view = 'add';
		} else return;
		event.preventDefault();
	}

	function onAddKey(event: KeyboardEvent) {
		if (event.key === 'ArrowDown') active = Math.min(flat.length - 1, active + 1);
		else if (event.key === 'ArrowUp') active = Math.max(0, active - 1);
		else if (event.key === 'Enter') {
			const def = flat[active];
			if (def) onpick(def);
		} else if (event.key === 'ArrowLeft' && search === '' && entries.length > 0) view = 'root';
		else return;
		event.preventDefault();
	}

	function onKey(event: KeyboardEvent) {
		if (event.key === 'Escape') {
			event.preventDefault();
			if (view === 'add' && entries.length > 0 && !startInAdd) view = 'root';
			else onclose();
			return;
		}
		if (event.key === 'Tab') {
			event.preventDefault();
			return;
		}
		if (view === 'root') onRootKey(event);
		else onAddKey(event);
	}
</script>

<svelte:window onpointerdown={onWindowPointer} onblur={onclose} />

<div
	bind:this={container}
	role="menu"
	tabindex="-1"
	aria-label={view === 'add' ? 'Add a block' : 'Canvas menu'}
	class={sheet
		? 'fixed inset-x-0 bottom-0 z-50 flex max-h-3/4 flex-col overflow-hidden rounded-t-xl border-t border-border bg-popover text-popover-foreground shadow-lg'
		: 'fixed z-50 flex max-h-[min(30rem,calc(100vh-1rem))] w-72 flex-col overflow-hidden rounded-lg border border-border bg-popover text-popover-foreground shadow-lg'}
	style:left={sheet ? undefined : `${position.left}px`}
	style:top={sheet ? undefined : `${position.top}px`}
	onkeydown={onKey}
	oncontextmenu={(event) => event.preventDefault()}
	data-testid="canvas-menu"
	data-presentation={presentation}
>
	{#if sheet}
		<div class="mx-auto mt-2 h-1 w-10 shrink-0 rounded-full bg-muted" aria-hidden="true"></div>
	{/if}
	{#if view === 'root'}
		<div class="flex flex-col p-1">
			{#each entries as entry, index (index)}
				{#if entry.kind === 'separator'}
					<div class="my-1 h-px bg-border" role="separator"></div>
				{:else if entry.kind === 'add'}
					{@const Icon = entry.icon}
					<button
						type="button"
						role="menuitem"
						data-add="true"
						aria-haspopup="menu"
						class="flex items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm outline-none hover:bg-accent hover:text-accent-foreground focus-visible:bg-accent focus-visible:text-accent-foreground"
						onclick={() => (view = 'add')}
					>
						{#if Icon}<Icon class="size-4 text-muted-foreground" />{/if}
						<span class="flex-1">{entry.label}</span>
						<ChevronRightIcon class="size-4 text-muted-foreground" />
					</button>
				{:else}
					{@const Icon = entry.icon}
					<button
						type="button"
						role="menuitem"
						disabled={entry.disabled}
						class={[
							'flex items-center gap-2 rounded-md px-2 py-1.5 text-left text-sm outline-none hover:bg-accent hover:text-accent-foreground focus-visible:bg-accent focus-visible:text-accent-foreground disabled:pointer-events-none disabled:opacity-50',
							entry.danger && 'text-destructive'
						]}
						onclick={() => {
							entry.onselect();
							onclose();
						}}
					>
						{#if Icon}<Icon class="size-4 text-muted-foreground" />{/if}
						<span class="flex-1">{entry.label}</span>
						{#if entry.shortcut}
							<span class="text-xs text-muted-foreground">{entry.shortcut}</span>
						{/if}
					</button>
				{/if}
			{/each}
		</div>
	{:else}
		<div class="flex items-center gap-1.5 border-b border-border px-2 py-1.5">
			{#if entries.length > 0 && !startInAdd}
				<button
					type="button"
					class="rounded-sm p-0.5 text-muted-foreground hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
					aria-label="Back"
					onclick={() => (view = 'root')}><ChevronLeftIcon class="size-4" /></button
				>
			{:else}
				<SearchIcon class="size-4 text-muted-foreground" />
			{/if}
			<input
				bind:this={searchInput}
				bind:value={search}
				type="text"
				aria-label="Search blocks"
				placeholder="Search blocks…"
				class="min-w-0 flex-1 bg-transparent text-base outline-none placeholder:text-muted-foreground sm:text-sm"
			/>
			{#if sheet}
				<button
					type="button"
					class="rounded-sm p-1 text-muted-foreground hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
					aria-label="Close"
					onclick={onclose}><XIcon class="size-4" /></button
				>
			{/if}
		</div>
		<div class="min-h-0 flex-1 overflow-y-auto p-1" role="listbox" aria-label="Blocks">
			{#each groups as group (group.category)}
				<p class="px-2 pt-2 pb-1 text-3xs tracking-caps text-muted-foreground uppercase">
					{group.label}
				</p>
				{#each group.items as def (def.id)}
					{@const Icon = iconForNode(def.id, def.category)}
					{@const index = flat.indexOf(def)}
					<button
						type="button"
						role="option"
						tabindex="-1"
						aria-selected={index === active}
						data-index={index}
						class={[
							'flex w-full items-start gap-2 rounded-md px-2 py-1.5 text-left outline-none hover:bg-accent hover:text-accent-foreground',
							index === active && 'bg-accent text-accent-foreground'
						]}
						onmousemove={() => (active = index)}
						onclick={() => onpick(def)}
					>
						<span
							class={[
								'mt-0.5 flex size-6 shrink-0 items-center justify-center rounded-md',
								toneOf(def.category) === 'trigger' && 'bg-success/15 text-success',
								toneOf(def.category) === 'data' && 'bg-info/15 text-info',
								toneOf(def.category) === 'action' && 'bg-primary/15 text-primary',
								toneOf(def.category) === 'ai' && 'bg-chart-5/15 text-chart-5',
								toneOf(def.category) === 'logic' && 'bg-chart-2/15 text-chart-2',
								toneOf(def.category) === 'integration' &&
									'bg-chart-4/15 text-chart-4'
							]}><Icon class="size-3.5" /></span
						>
						<span class="min-w-0">
							<span class="block text-sm leading-tight">{def.label}</span>
							<span
								class="line-clamp-2 block text-xs leading-snug text-muted-foreground"
								>{def.description}</span
							>
						</span>
					</button>
				{/each}
			{/each}
			{#if flat.length === 0}
				<p class="px-3 py-6 text-center text-sm text-muted-foreground">
					{accepts
						? 'Nothing can follow this step with that kind of information.'
						: 'No block matches your search.'}
				</p>
			{/if}
		</div>
	{/if}
</div>
