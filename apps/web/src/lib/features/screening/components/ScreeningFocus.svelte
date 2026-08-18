<script lang="ts">
	import {
		createGetProjectProtocol,
		createListTitleAbstractScreeningQueue,
		createScreenReport,
		getListTitleAbstractScreeningQueueQueryKey
	} from '$lib/api/generated/review/review';
	import type { ScreeningDecisionInput } from '$lib/api/generated/models';
	import * as Alert from '$lib/components/ui/alert';
	import { Badge } from '$lib/components/ui/badge';
	import { Button } from '$lib/components/ui/button';
	import * as Card from '$lib/components/ui/card';
	import * as Empty from '$lib/components/ui/empty';
	import { Skeleton } from '$lib/components/ui/skeleton';
	import { useQueryClient } from '@tanstack/svelte-query';
	import { ChevronLeft, ChevronRight, FileText, ShieldCheck } from '@lucide/svelte';
	import DecisionBar from './DecisionBar.svelte';

	let { projectId }: { projectId: string } = $props();
	const queryClient = useQueryClient();
	const protocolQuery = createGetProjectProtocol(() => projectId);
	const queueQuery = createListTitleAbstractScreeningQueue(
		() => projectId,
		() => ({ status: 'unscreened', limit: 100 })
	);
	const screenReport = createScreenReport();

	let selectedIndex = $state(0);
	const queue = $derived(queueQuery.data?.data.items ?? []);
	const current = $derived(queue[selectedIndex] ?? null);
	const protocol = $derived(protocolQuery.data?.data);
	const criteria = $derived(Array.isArray(protocol?.criteria) ? protocol.criteria : []) as Array<{
		id?: string;
		label?: string;
		description?: string;
	}>;
	const pendingCount = $derived(queueQuery.data?.data.total ?? 0);
	const errorMessage = $derived(
		screenReport.error?.message ?? queueQuery.error?.message ?? protocolQuery.error?.message
	);

	function selectPrevious() {
		if (queue.length === 0) return;
		selectedIndex = selectedIndex === 0 ? queue.length - 1 : selectedIndex - 1;
	}

	function selectNext() {
		if (queue.length === 0) return;
		selectedIndex = selectedIndex === queue.length - 1 ? 0 : selectedIndex + 1;
	}

	async function decide(decision: ScreeningDecisionInput) {
		if (!current || !protocol || screenReport.isPending) return;
		try {
			await screenReport.mutateAsync({
				projectId,
				reportId: current.report_id,
				data: {
					stage: 'title_abstract',
					decision,
					protocol_version_id: protocol.id,
					expected_revision: current.revision
				}
			});
			selectedIndex = 0;
			await queryClient.invalidateQueries({
				queryKey: getListTitleAbstractScreeningQueueQueryKey(projectId, {
					status: 'unscreened',
					limit: 100
				})
			});
			await queueQuery.refetch();
		} catch {
			// The mutation error is rendered above the queue.
		}
	}

	function handleKeydown(event: KeyboardEvent) {
		const target = event.target;
		if (
			target instanceof HTMLElement &&
			target.closest('input, textarea, button, [contenteditable="true"]')
		) {
			return;
		}
		if (event.key.toLowerCase() === 'i') void decide('include');
		if (event.key.toLowerCase() === 'e') void decide('exclude');
		if (event.key.toLowerCase() === 'm') void decide('maybe');
		if (event.key === 'ArrowLeft') selectPrevious();
		if (event.key === 'ArrowRight') selectNext();
	}
</script>

<svelte:window onkeydown={handleKeydown} />

<div class="mx-auto flex w-full max-w-7xl flex-col gap-6 p-4 md:p-8">
	<header class="flex flex-col gap-4 md:flex-row md:items-end md:justify-between">
		<div class="flex flex-col gap-2">
			<div class="flex items-center gap-2 text-sm text-muted-foreground">
				<ShieldCheck data-icon="inline-start" />
				Evidence workspace / title & abstract
			</div>
			<h1 class="text-3xl font-semibold tracking-tight">Screen reports</h1>
			<p class="max-w-2xl text-muted-foreground">
				Make protocol-grounded, reversible decisions. Every decision is versioned and
				remains project-specific.
			</p>
		</div>
		<div class="flex items-center gap-2">
			<Badge variant="secondary">{pendingCount} remaining</Badge>
			{#if protocol}
				<Badge variant="outline">Protocol v{protocol.version}</Badge>
			{/if}
		</div>
	</header>

	{#if errorMessage}
		<Alert.Root variant="destructive">
			<Alert.Title>Screening could not continue</Alert.Title>
			<Alert.Description>{errorMessage}</Alert.Description>
		</Alert.Root>
	{/if}

	<div class="grid gap-6 lg:grid-cols-[minmax(0,1fr)_20rem]">
		<Card.Root>
			<Card.Header class="flex-row items-center justify-between gap-4">
				<div class="flex flex-col gap-1">
					<Card.Title>Focus mode</Card.Title>
					<Card.Description>
						{#if current}
							Report {selectedIndex + 1} of {queue.length}
						{:else}
							Your title/abstract queue
						{/if}
					</Card.Description>
				</div>
				<div class="flex items-center gap-1">
					<Button
						variant="ghost"
						size="icon"
						aria-label="Previous report"
						onclick={selectPrevious}
					>
						<ChevronLeft />
					</Button>
					<Button
						variant="ghost"
						size="icon"
						aria-label="Next report"
						onclick={selectNext}
					>
						<ChevronRight />
					</Button>
				</div>
			</Card.Header>
			<Card.Content class="flex flex-col gap-6">
				{#if queueQuery.isPending}
					<div class="flex flex-col gap-3">
						<Skeleton class="h-8 w-4/5" />
						<Skeleton class="h-4 w-1/3" />
						<Skeleton class="h-32 w-full" />
					</div>
				{:else if !current}
					<Empty.Root>
						<Empty.Media variant="icon"><FileText /></Empty.Media>
						<Empty.Header>
							<Empty.Title>Queue complete</Empty.Title>
							<Empty.Description>
								There are no unscreened reports in this protocol stage.
							</Empty.Description>
						</Empty.Header>
					</Empty.Root>
				{:else}
					<article class="flex flex-col gap-5" aria-live="polite">
						<div class="flex flex-col gap-3">
							<div class="flex flex-wrap items-center gap-2">
								<Badge variant="outline"
									>{current.publication_year ?? 'Year unknown'}</Badge
								>
								{#if current.doi}<span
										class="font-mono text-xs text-muted-foreground"
										>{current.doi}</span
									>{/if}
							</div>
							<h2 class="text-2xl leading-tight font-semibold">
								{current.title ?? 'Untitled report'}
							</h2>
						</div>
						<div class="rounded-lg border bg-muted/30 p-4">
							<p class="text-sm leading-7 whitespace-pre-wrap text-foreground/90">
								{current.abstract_text ??
									'No abstract is available. Use Maybe when the available evidence is insufficient.'}
							</p>
						</div>
						<DecisionBar
							disabled={!protocol}
							pending={screenReport.isPending}
							onDecision={decide}
						/>
					</article>
				{/if}
			</Card.Content>
		</Card.Root>

		<aside class="flex flex-col gap-6">
			<Card.Root>
				<Card.Header>
					<Card.Title>Eligibility criteria</Card.Title>
					<Card.Description
						>Protocol v{protocol?.version ?? '—'} · cite the reason in your notes when uncertain.</Card.Description
					>
				</Card.Header>
				<Card.Content>
					{#if criteria.length > 0}
						<ol class="flex flex-col gap-4">
							{#each criteria as criterion (criterion.id ?? criterion.label)}
								<li class="flex flex-col gap-1">
									<span class="text-sm font-medium">{criterion.label}</span>
									<span class="text-sm text-muted-foreground"
										>{criterion.description}</span
									>
								</li>
							{/each}
						</ol>
					{:else}
						<p class="text-sm text-muted-foreground">
							No criteria are published for this protocol.
						</p>
					{/if}
				</Card.Content>
			</Card.Root>

			<Card.Root>
				<Card.Header>
					<Card.Title>Review guardrails</Card.Title>
				</Card.Header>
				<Card.Content class="flex flex-col gap-3 text-sm text-muted-foreground">
					<p>Maybe is distinct from Include and remains visible for follow-up.</p>
					<p>
						Decisions are append-only and tied to protocol v{protocol?.version ?? '—'}.
					</p>
					<p>AI suggestions, when enabled, arrive as proposals for human review.</p>
				</Card.Content>
			</Card.Root>
		</aside>
	</div>
</div>
