<script lang="ts">
	import { Input } from '@deepref/ui/input';
	import { Label } from '@deepref/ui/label';
	import { composeQueryTerms, type PublicationQuery } from '../model';

	let { value, onchange }: { value: unknown; onchange: (value: PublicationQuery) => void } =
		$props();

	const query = $derived<PublicationQuery>(
		typeof value === 'object' && value !== null ? (value as PublicationQuery) : { terms: '' }
	);

	function split(raw: string): string[] {
		return raw
			.split(',')
			.map((part) => part.trim())
			.filter(Boolean);
	}
	function update(change: Partial<PublicationQuery>) {
		const next = { ...query, ...change };
		// Older automations only have the combined words; keep them until the
		// person fills in the simple fields.
		const hasParts =
			(next.keywords?.length ?? 0) > 0 ||
			(next.authors?.length ?? 0) > 0 ||
			!!next.journal?.trim();
		onchange({
			...next,
			terms: hasParts ? composeQueryTerms(next) : (change.terms ?? query.terms ?? '')
		});
	}
</script>

<div class="flex flex-col gap-3" data-testid="publication-query-field">
	<div class="flex flex-col gap-1.5">
		<Label for="alert-keywords">Keywords</Label>
		<Input
			id="alert-keywords"
			placeholder="e.g. sleep, exercise"
			value={(query.keywords ?? []).join(', ')}
			onchange={(event) => update({ keywords: split(event.currentTarget.value) })}
		/>
		<p class="text-xs text-muted-foreground">
			Separate several with commas. Any of them can match.
		</p>
	</div>
	<div class="flex flex-col gap-1.5">
		<Label for="alert-authors">Authors</Label>
		<Input
			id="alert-authors"
			placeholder="e.g. Silva J"
			value={(query.authors ?? []).join(', ')}
			onchange={(event) => update({ authors: split(event.currentTarget.value) })}
		/>
	</div>
	<div class="flex flex-col gap-1.5">
		<Label for="alert-journal">Journal</Label>
		<Input
			id="alert-journal"
			placeholder="e.g. The Lancet"
			value={query.journal ?? ''}
			onchange={(event) => update({ journal: event.currentTarget.value })}
		/>
	</div>
	{#if query.terms && !(query.keywords?.length || query.authors?.length || query.journal)}
		<p class="rounded-md bg-muted px-2 py-1.5 text-xs text-muted-foreground">
			Currently looking for: {query.terms}
		</p>
	{/if}
</div>
