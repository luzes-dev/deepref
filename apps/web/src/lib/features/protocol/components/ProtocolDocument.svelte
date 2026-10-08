<script lang="ts">
	import { createListProjectReviewProtocolVersions, type ProtocolDto } from '../api';
	import { Badge } from '@deepref/ui/badge';
	import { Button } from '@deepref/ui/button';
	import {
		FRAMEWORK_FIELDS,
		criterionDimensionLabel,
		criterionStageLabel,
		frameworkLabel,
		humanizeKey
	} from '../codecs';
	import { diffProtocols } from '../diff';

	let {
		protocol,
		projectId,
		onAmend
	}: { protocol: ProtocolDto; projectId: string; onAmend: () => void } = $props();

	const versionsQuery = createListProjectReviewProtocolVersions(() => projectId);
	const versions = $derived(
		(versionsQuery.data?.data.items ?? []).toSorted((a, b) => b.version - a.version)
	);
	const versionRows = $derived(
		versions.map((version, index) => {
			const previous = versions[index + 1];
			return { version, changes: previous ? diffProtocols(previous, version) : undefined };
		})
	);

	function publishedOn(value: string | null | undefined): string {
		return value
			? new Date(value).toLocaleDateString(undefined, { dateStyle: 'medium' })
			: 'not published';
	}

	const frameworkEntries = $derived.by(() => {
		const order: readonly string[] =
			protocol.framework_kind in FRAMEWORK_FIELDS
				? FRAMEWORK_FIELDS[protocol.framework_kind as keyof typeof FRAMEWORK_FIELDS]
				: [];
		const rank = (key: string) => (order.includes(key) ? order.indexOf(key) : order.length);
		return Object.entries(protocol.framework_fields ?? {}).toSorted(
			([a], [b]) => rank(a) - rank(b)
		);
	});
	const criteriaGroups = $derived([
		['Include when', protocol.criteria.filter((criterion) => criterion.kind !== 'exclusion')],
		['Exclude when', protocol.criteria.filter((criterion) => criterion.kind === 'exclusion')]
	] as const);
</script>

<article class="flex max-w-4xl flex-col gap-10" data-testid="protocol-document">
	<header class="flex flex-wrap items-start justify-between gap-4">
		<div class="flex min-w-0 flex-col gap-1">
			<p class="text-xs text-muted-foreground">
				Version {protocol.version} · Published{protocol.published_at
					? ` ${new Date(protocol.published_at).toLocaleDateString(undefined, { dateStyle: 'medium' })}`
					: ''}
			</p>
			<h2 class="editorial-title text-2xl">{protocol.name}</h2>
		</div>
		<Button variant="outline" onclick={onAmend}>Amend published version</Button>
	</header>

	<section class="flex flex-col gap-2" aria-labelledby="protocol-question">
		<h3
			id="protocol-question"
			class="text-2xs font-semibold tracking-caps text-muted-foreground uppercase"
		>
			Question
		</h3>
		<p class="text-lg leading-8">{protocol.question}</p>
		{#if protocol.objective}<p class="leading-7 text-muted-foreground">
				{protocol.objective}
			</p>{/if}
	</section>

	<section class="flex flex-col gap-3" aria-labelledby="protocol-framework">
		<h3
			id="protocol-framework"
			class="text-2xs font-semibold tracking-caps text-muted-foreground uppercase"
		>
			Framework · {frameworkLabel(protocol.framework_kind)}
		</h3>
		<dl class="grid gap-x-8 gap-y-3 sm:grid-cols-[10rem_minmax(0,1fr)]">
			{#each frameworkEntries as [key, value] (key)}
				<dt class="text-sm font-medium">{humanizeKey(key)}</dt>
				<dd class="text-sm leading-6 text-muted-foreground">{value}</dd>
			{/each}
		</dl>
	</section>

	<section class="flex flex-col gap-4" aria-labelledby="protocol-criteria">
		<h3
			id="protocol-criteria"
			class="text-2xs font-semibold tracking-caps text-muted-foreground uppercase"
		>
			Eligibility criteria
		</h3>
		<div class="grid gap-8 md:grid-cols-2">
			{#each criteriaGroups as [heading, items] (heading)}
				<div class="flex flex-col gap-3">
					<p class="text-sm font-semibold">{heading}</p>
					{#each items as criterion (criterion.id)}
						<div class="flex flex-col gap-0.5">
							<span class="text-sm font-medium">{criterion.label}</span>
							<span class="text-sm leading-6 text-muted-foreground"
								>{criterion.description}</span
							>
							<span class="text-xs text-muted-foreground"
								>{criterionDimensionLabel(criterion.dimension)} · {criterionStageLabel(
									criterion.stage
								)}</span
							>
						</div>
					{:else}
						<p class="text-sm text-muted-foreground">None.</p>
					{/each}
				</div>
			{/each}
		</div>
	</section>

	{#if versions.length > 1}
		<details class="disclosure" data-testid="protocol-versions">
			<summary class="cursor-pointer text-sm font-medium">
				Versions ({versions.length})
			</summary>
			<ol class="mt-3 flex flex-col gap-4">
				{#each versionRows as { version, changes } (version.id)}
					<li class="flex flex-col gap-1">
						<p class="flex items-center gap-2 text-sm font-medium">
							Version {version.version}
							<span class="text-xs font-normal text-muted-foreground"
								>{publishedOn(version.published_at)}</span
							>
							{#if version.id === protocol.id}<Badge variant="secondary" size="xs"
									>Current</Badge
								>{/if}
						</p>
						{#if !changes}
							<p class="text-sm text-muted-foreground">First published version.</p>
						{:else if changes.length === 0}
							<p class="text-sm text-muted-foreground">
								No changes from the previous version.
							</p>
						{:else}
							<ul class="flex flex-col gap-0.5 text-sm text-muted-foreground">
								{#each changes as change (change.kind + change.subject)}
									<li>
										<span class="font-medium text-foreground"
											>{change.kind === 'added'
												? 'Added'
												: change.kind === 'removed'
													? 'Removed'
													: 'Changed'}</span
										>
										{change.subject}{change.detail ? ` (${change.detail})` : ''}
									</li>
								{/each}
							</ul>
						{/if}
					</li>
				{/each}
			</ol>
		</details>
	{/if}

	<p class="text-xs text-muted-foreground">
		Screening decisions stay tied to this version. Amending starts a new draft; this record does
		not change.
	</p>
</article>
