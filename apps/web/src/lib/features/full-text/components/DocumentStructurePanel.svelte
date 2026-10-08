<script lang="ts">
	import type {
		DocumentBlockDto,
		DocumentReferenceDto,
		DocumentSectionDto
	} from '$lib/api/generated/models';
	import * as Alert from '@deepref/ui/alert';
	import { Skeleton } from '@deepref/ui/skeleton';
	import { ExternalLink } from '@lucide/svelte';
	import { doiHref, firstBlockOfSection, outlineIndentClass } from '../structure';

	let {
		view,
		sections,
		references,
		blocks,
		loading,
		errorMessage,
		onSelectSection
	}: {
		view: 'outline' | 'references';
		sections: DocumentSectionDto[];
		references: DocumentReferenceDto[];
		blocks: DocumentBlockDto[];
		loading: boolean;
		errorMessage: string;
		/** Opens the reader at the first evidence block of a section. */
		onSelectSection: (block: DocumentBlockDto) => void;
	} = $props();

	const sectionTargets = $derived(
		new Map(sections.map((section) => [section.id, firstBlockOfSection(section, blocks)]))
	);
</script>

<!-- eslint-disable svelte/no-navigation-without-resolve -- the only link here is an external DOI resolver, not app navigation -->
{#if loading}
	<div class="flex flex-col gap-2 p-6" role="status" aria-label="Loading parsed structure">
		<Skeleton class="h-8 w-full" /><Skeleton class="h-8 w-5/6" /><Skeleton class="h-8 w-4/6" />
	</div>
{:else if errorMessage}
	<div class="p-6">
		<Alert.Root variant="destructive">
			<Alert.Title>Parsed structure is unavailable</Alert.Title>
			<Alert.Description>{errorMessage}</Alert.Description>
		</Alert.Root>
	</div>
{:else if view === 'outline'}
	<div class="flex flex-col gap-3 p-6">
		<p class="text-xs text-muted-foreground">
			Sections found in the PDF. Selecting one opens the reader at its first passage.
		</p>
		{#if sections.length === 0}
			<p class="text-sm text-muted-foreground">
				No sections were parsed for this document. Re-parse the PDF to try again.
			</p>
		{:else}
			<ol class="flex flex-col gap-1" aria-label="Parsed outline">
				{#each sections as section (section.id)}
					{@const target = sectionTargets.get(section.id)}
					<li class={outlineIndentClass(section.depth)}>
						<button
							type="button"
							class="flex w-full min-w-0 items-baseline gap-2 rounded-md px-2 py-1.5 text-left text-sm transition-colors hover:bg-muted focus-visible:outline-2 focus-visible:outline-ring disabled:text-muted-foreground"
							title={section.path.join(' › ')}
							disabled={!target}
							onclick={() => target && onSelectSection(target)}
						>
							{#if section.number}<span
									class="shrink-0 text-xs text-muted-foreground tabular-nums"
									>{section.number}</span
								>{/if}
							<span class="min-w-0 break-words">{section.title}</span>
							{#if target}<span
									class="ml-auto shrink-0 text-xs text-muted-foreground tabular-nums"
									>p. {target.page_number}</span
								>{/if}
						</button>
					</li>
				{/each}
			</ol>
		{/if}
	</div>
{:else}
	<div class="flex flex-col gap-3 p-6">
		<p class="text-xs text-muted-foreground">
			References parsed from the PDF. DOIs link to the publisher record.
		</p>
		{#if references.length === 0}
			<p class="text-sm text-muted-foreground">
				No references were parsed for this document.
			</p>
		{:else}
			<ol class="flex flex-col gap-2" aria-label="Parsed references">
				{#each references as reference (reference.id)}
					{@const href = doiHref(reference.doi)}
					<li class="rounded-lg border bg-card p-3 text-sm">
						<p class="leading-6 break-words">{reference.raw}</p>
						{#if reference.year || reference.venue || href}
							<p
								class="mt-2 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-muted-foreground"
							>
								{#if reference.year}<span class="tabular-nums"
										>{reference.year}</span
									>{/if}
								{#if reference.venue}<span class="break-words"
										>{reference.venue}</span
									>{/if}
								{#if href}
									<a
										class="inline-flex items-center gap-1 break-all text-primary underline underline-offset-4"
										{href}
										target="_blank"
										rel="noopener noreferrer"
										>DOI {reference.doi}<ExternalLink
											class="size-3 shrink-0"
											aria-hidden="true"
										/></a
									>
								{/if}
							</p>
						{/if}
					</li>
				{/each}
			</ol>
		{/if}
	</div>
{/if}
