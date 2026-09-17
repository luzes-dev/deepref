<script lang="ts">
	import type { AutomationDefinitionDto } from '$lib/api/generated/models';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import BookOpenIcon from '@lucide/svelte/icons/book-open';
	import CheckCircle2Icon from '@lucide/svelte/icons/check-circle-2';
	import ClipboardCheckIcon from '@lucide/svelte/icons/clipboard-check';
	import CopyIcon from '@lucide/svelte/icons/copy';
	import FileCheckIcon from '@lucide/svelte/icons/file-check';
	import FileSearchIcon from '@lucide/svelte/icons/file-search';
	import FileSpreadsheetIcon from '@lucide/svelte/icons/file-spreadsheet';
	import FileTextIcon from '@lucide/svelte/icons/file-text';
	import FolderTreeIcon from '@lucide/svelte/icons/folder-tree';
	import GitForkIcon from '@lucide/svelte/icons/git-fork';
	import LayersIcon from '@lucide/svelte/icons/layers';
	import PlayIcon from '@lucide/svelte/icons/play';
	import SearchIcon from '@lucide/svelte/icons/search';
	import ShieldAlertIcon from '@lucide/svelte/icons/shield-alert';
	import ShieldCheckIcon from '@lucide/svelte/icons/shield-check';
	import TagsIcon from '@lucide/svelte/icons/tags';
	import WrenchIcon from '@lucide/svelte/icons/wrench';
	import type { Component } from 'svelte';
	import {
		PREDEFINED_RECIPES,
		RECIPE_CATEGORIES,
		type PredefinedRecipe,
		type RecipeCategory
	} from '../recipes';
	import RecipeLaunchModal from './RecipeLaunchModal.svelte';

	let {
		projectId,
		definitions,
		onFork,
		onDefinitionCreated,
		onRunQueued
	}: {
		projectId: string;
		definitions: readonly AutomationDefinitionDto[];
		onFork: (recipe: PredefinedRecipe) => void;
		onDefinitionCreated: () => Promise<void> | void;
		onRunQueued: (runId: string) => void;
	} = $props();

	const RECIPE_ICONS: Record<string, Component<{ class?: string }>> = {
		CheckCircle2: CheckCircle2Icon,
		Copy: CopyIcon,
		FileCheck: FileCheckIcon,
		FolderTree: FolderTreeIcon,
		Tags: TagsIcon,
		ShieldAlert: ShieldAlertIcon,
		BookOpen: BookOpenIcon,
		ClipboardCheck: ClipboardCheckIcon,
		FileSpreadsheet: FileSpreadsheetIcon,
		FileSearch: FileSearchIcon,
		Search: SearchIcon,
		Layers: LayersIcon,
		FileText: FileTextIcon,
		ShieldCheck: ShieldCheckIcon
	};

	const CATEGORY_DESCRIPTIONS: Record<RecipeCategory, string> = {
		Screening: 'Screening decisions and duplicate reviews, always queued for human sign-off.',
		'Studies & Synthesis':
			'Group reports into studies, classify designs, and prefill critical appraisal.',
		'Document Analysis':
			'Inspect documents, reports, and full-text blocks already attached to this project.',
		Maintenance: 'Read-only audits of project protocol state and configuration.'
	};

	let launchingRecipe = $state<PredefinedRecipe | null>(null);
	let launchOpen = $state(false);
	let launchedRunId = $state<string | null>(null);

	$effect(() => {
		if (!launchOpen && launchedRunId) {
			const runId = launchedRunId;
			launchedRunId = null;
			launchingRecipe = null;
			onRunQueued(runId);
		}
	});

	function recipeIcon(recipe: PredefinedRecipe): Component<{ class?: string }> {
		return RECIPE_ICONS[recipe.icon] ?? WrenchIcon;
	}

	function recipesFor(category: RecipeCategory): readonly PredefinedRecipe[] {
		return PREDEFINED_RECIPES.filter((recipe) => recipe.category === category);
	}

	function handleLaunched(runId: string): void {
		launchedRunId = runId;
	}
</script>

<div class="flex flex-col gap-8" data-testid="recipe-library">
	{#each RECIPE_CATEGORIES as category, categoryIndex (category)}
		{@const recipes = recipesFor(category)}
		<section class="flex flex-col gap-3" aria-labelledby={`recipe-category-${categoryIndex}`}>
			<div>
				<h2
					id={`recipe-category-${categoryIndex}`}
					class="text-base font-semibold text-foreground"
				>
					{category}
				</h2>
				<p class="mt-1 text-sm text-muted-foreground">
					{CATEGORY_DESCRIPTIONS[category]}
				</p>
			</div>
			<div class="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
				{#each recipes as recipe (recipe.id)}
					{@const Icon = recipeIcon(recipe)}
					<article
						class="group flex flex-col gap-3 rounded-xl border border-border/80 bg-card p-4 transition-colors hover:bg-muted/25"
						data-testid={`recipe-card-${recipe.id}`}
					>
						<div class="flex items-start justify-between gap-2">
							<span
								class="flex size-9 shrink-0 items-center justify-center rounded-lg bg-primary/10 text-primary"
							>
								<Icon class="size-4" />
							</span>
							<Badge variant="outline" class="text-[11px]">{recipe.category}</Badge>
						</div>
						<div class="min-w-0">
							<h3 class="font-medium text-foreground">{recipe.title}</h3>
							<p class="mt-1 text-sm leading-relaxed text-muted-foreground">
								{recipe.description}
							</p>
						</div>
						<div class="mt-auto flex items-center gap-2 pt-1">
							<Button
								size="sm"
								onclick={() => (launchingRecipe = recipe)}
								data-testid={`recipe-run-${recipe.id}`}
							>
								<PlayIcon data-icon="inline-start" aria-hidden="true" />
								{recipe.reviewDestination === null
									? 'Run inspection'
									: 'Run recipe'}
							</Button>
							<Button
								variant="ghost"
								size="sm"
								onclick={() => onFork(recipe)}
								data-testid={`recipe-fork-${recipe.id}`}
							>
								<GitForkIcon data-icon="inline-start" aria-hidden="true" />
								Fork
							</Button>
						</div>
					</article>
				{/each}
			</div>
		</section>
	{/each}
</div>

<RecipeLaunchModal
	bind:open={launchOpen}
	{projectId}
	recipe={launchingRecipe}
	{definitions}
	onLaunched={handleLaunched}
	{onDefinitionCreated}
/>
