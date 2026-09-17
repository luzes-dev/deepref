<script lang="ts">
	import * as Table from '../../src/primitives/table';
	import { Input } from '../../src/primitives/input';
	import { Badge } from '../../src/primitives/badge';
	import { Checkbox } from '../../src/primitives/checkbox';
	import { PageToolbar } from '../../src/patterns/page-toolbar';
	let query = $state('');
	let selected = $state<string[]>(['REF-001']);
	const rows = [
		{ id: 'REF-001', title: 'A comparison of methods for assessing the reliability of published information', owner: 'M. Alvarez', date: '14 Sep 2026', status: 'Complete' },
		{ id: 'REF-002', title: 'Longitudinal observations from a multi-site evaluation with supplementary methods and supporting documentation', owner: 'J. Chen', date: '12 Sep 2026', status: 'Needs attention' },
		{ id: 'REF-003', title: 'An annotated collection of reference materials', owner: 'A. Martins', date: '10 Sep 2026', status: 'Draft' },
		{ id: 'REF-004', title: 'Data quality report: unresolved identifiers and missing source files', owner: 'S. Patel', date: '08 Sep 2026', status: 'Failed' }
	];
	const filtered = $derived(rows.filter(row => `${row.id} ${row.title} ${row.owner}`.toLowerCase().includes(query.toLowerCase())));
	const allSelected = $derived(filtered.length > 0 && filtered.every(row => selected.includes(row.id)));
	const someSelected = $derived(filtered.some(row => selected.includes(row.id)));
	function toggle(id: string) { selected = selected.includes(id) ? selected.filter(value => value !== id) : [...selected, id]; }
</script>
<div class="story-sheet">
	<header><h1>Dense records</h1><p>Titles wrap; identifiers and dates stay aligned. At narrow widths the named table region scrolls horizontally and can receive keyboard focus.</p></header>
	<PageToolbar label="Record filters"><Input aria-label="Filter records" placeholder="Filter by title, owner, or identifier" bind:value={query} class="w-full sm:w-80" />{#snippet trailing()}<span class="text-xs text-muted-foreground" aria-live="polite">{selected.length} selected · {filtered.length} results</span>{/snippet}</PageToolbar>
	<Table.Root containerLabel="Reference records" class="min-w-[640px]">
		<Table.Header><Table.Row><Table.Head class="w-10"><Checkbox aria-label="Select all visible records" checked={allSelected} indeterminate={someSelected && !allSelected} onCheckedChange={() => selected = allSelected ? selected.filter(id => !filtered.some(row => row.id === id)) : [...new Set([...selected, ...filtered.map(row => row.id)])]} /></Table.Head><Table.Head>Record</Table.Head><Table.Head>Owner</Table.Head><Table.Head>Updated</Table.Head><Table.Head>Status</Table.Head></Table.Row></Table.Header>
		<Table.Body>{#each filtered as row (row.id)}<Table.Row data-state={selected.includes(row.id) ? 'selected' : undefined}><Table.Cell><Checkbox aria-label={`Select ${row.id}`} checked={selected.includes(row.id)} onCheckedChange={() => toggle(row.id)} /></Table.Cell><Table.Cell class="max-w-sm whitespace-normal"><div class="font-medium">{row.title}</div><div class="mt-1 font-mono text-xs text-muted-foreground">{row.id}</div></Table.Cell><Table.Cell>{row.owner}</Table.Cell><Table.Cell class="text-xs tabular-nums">{row.date}</Table.Cell><Table.Cell><Badge variant={row.status === 'Complete' ? 'success' : row.status === 'Needs attention' ? 'warning' : row.status === 'Failed' ? 'destructive' : 'secondary'}>{row.status}</Badge></Table.Cell></Table.Row>{:else}<Table.Row><Table.Cell colspan={5} class="h-32 text-center">No records match “{query}”. Try a shorter search.</Table.Cell></Table.Row>{/each}</Table.Body>
	</Table.Root>
</div>
