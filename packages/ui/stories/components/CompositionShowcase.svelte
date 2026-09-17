<script lang="ts">
	import { PageHeader } from '../../src/patterns/page-header';
	import { PageToolbar } from '../../src/patterns/page-toolbar';
	import { Surface } from '../../src/patterns/surface';
	import { MetricTile } from '../../src/patterns/metric-tile';
	import { StatePanel } from '../../src/patterns/state-panel';
	import { Button } from '../../src/primitives/button';
	import { Input } from '../../src/primitives/input';
	let { constrained = false } = $props<{ constrained?: boolean }>();
</script>
<div class="story-sheet" style:max-width={constrained ? '390px' : undefined}>
	<PageHeader title="A shared collection with a deliberately long descriptive title" description="Keep the current task, its context, and its actions distinct. Controls wrap without squeezing the heading.">
		{#snippet actions()}<Button>New record</Button><Button variant="outline">Export collection</Button><Button variant="ghost">Manage access</Button>{/snippet}
	</PageHeader>
	<PageToolbar><Input aria-label="Search collection" placeholder="Search collection" class="w-full sm:w-72" />{#snippet trailing()}<Button variant="outline" size="sm">Filters</Button><Button variant="outline" size="sm">View options</Button>{/snippet}</PageToolbar>
	<div class="grid grid-cols-2 gap-4 sm:grid-cols-3"><MetricTile label="Total records" value="1,248" detail="Across all sources" /><MetricTile label="Reviewed" value="986" detail="79% of collection" /><MetricTile label="Needs attention" value="12" tone="warning" /></div>
	<section><h2>Surface roles in composition</h2><div class="grid gap-4 sm:grid-cols-2"><Surface tone="plain" as="section" class="border-b border-border-subtle p-section"><h3 class="text-sm font-semibold">Plain / section</h3><p>Use for a grouped page section when spacing and separators should carry hierarchy without another card.</p></Surface><Surface as="section" label="Content surface" class="p-section"><h3 class="text-sm font-semibold">Content / default</h3><p>Use for a distinct reading or working surface. Padding belongs to the composition.</p></Surface><Surface tone="subtle" class="p-section"><h3 class="text-sm font-semibold">Supporting / subtle</h3><p>Secondary context shares the same typography without demanding another card border.</p></Surface><Surface tone="inset" class="p-section"><h3 class="text-sm font-semibold">Recessed / inset</h3><p>Logs, samples, and grouped supporting values.</p></Surface><Surface tone="raised" class="p-section"><h3 class="text-sm font-semibold">Temporary / raised</h3><p>Reserve elevation for content that sits above the working surface.</p></Surface></div></section>
	<StatePanel state="empty" title="No matching records" description="Your collection is still available. Clear the filters to see all records.">{#snippet action()}<Button variant="outline">Clear filters</Button>{/snippet}</StatePanel>
</div>
