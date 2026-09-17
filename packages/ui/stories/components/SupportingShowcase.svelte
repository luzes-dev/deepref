<script lang="ts">
	import { Label } from '../../src/primitives/label';
	import { Slider } from '../../src/primitives/slider';
	import { Progress } from '../../src/primitives/progress';
	import { Spinner } from '../../src/primitives/spinner';
	import { Skeleton } from '../../src/primitives/skeleton';
	import { TagsInput } from '../../src/primitives/tags-input';
	import { CopyButton } from '../../src/primitives/copy-button';
	import { ScrollArea } from '../../src/primitives/scroll-area';
	import * as NumberField from '../../src/primitives/number-field';
	import * as Resizable from '../../src/primitives/resizable';
	import * as Card from '../../src/primitives/card';
	import { PaginationLoadMore } from '../../src/patterns/pagination-load-more';
	let count = $state(10);
	let amount = $state(40);
	let tags = $state(['Reviewed']);
	let loaded = $state(20);
</script>
<div class="story-sheet">
	<header><h1>Supporting controls</h1><p>Small controls still need labels, keyboard feedback, and predictable bounds.</p></header>
	<section><h2>Numeric values and tags</h2><div class="grid gap-5 sm:grid-cols-2"><div class="flex flex-col gap-2"><Label for="batch-size">Batch size</Label><NumberField.Root bind:value={count} min={1} max={50}><NumberField.Group><NumberField.Decrement /><NumberField.Input id="batch-size" aria-label="Batch size" /><NumberField.Increment /></NumberField.Group></NumberField.Root></div><div class="flex flex-col gap-3"><Label>Threshold: {amount}%</Label><Slider type="single" bind:value={amount} thumbLabel="Threshold" min={0} max={100} /></div><div class="flex flex-col gap-2"><Label for="tags">Labels</Label><TagsInput id="tags" aria-label="Labels" bind:value={tags} suggestions={['Reviewed', 'Draft', 'Shared']} placeholder="Add a label" /></div><div class="flex items-center gap-3"><code class="text-xs">REF-00248</code><CopyButton text="REF-00248" /></div></div></section>
	<section><h2>Loading and progress</h2><div class="flex items-center gap-2 text-sm"><Spinner />Preparing records</div><Progress value={64} aria-label="Export progress" /><p>64 of 100 records prepared.</p><div class="flex max-w-lg flex-col gap-2" aria-label="Loading record preview" role="status"><span class="sr-only">Loading record preview</span><Skeleton class="h-4 w-2/3" /><Skeleton class="h-3 w-full" /><Skeleton class="h-3 w-4/5" /></div></section>
	<section><h2>Bounded panels</h2><Resizable.PaneGroup direction="horizontal" class="min-h-40 rounded-md border"><Resizable.Pane defaultSize={45} minSize={25}><div class="p-4 text-sm">Drag or keyboard-adjust the divider.</div></Resizable.Pane><Resizable.Handle withHandle aria-label="Resize preview" /><Resizable.Pane defaultSize={55} minSize={25}><ScrollArea class="h-40"><div class="flex flex-col gap-3 p-4">{#each Array.from({length: 12}, (_, i) => i + 1) as row (row)}<p class="text-sm">Supporting detail {row}</p>{/each}</div></ScrollArea></Resizable.Pane></Resizable.PaneGroup></section>
	<section><h2>Content card</h2><Card.Root><Card.Header><Card.Title>Export summary</Card.Title><Card.Description>A card groups a distinct object; avoid nesting cards for every section.</Card.Description></Card.Header><Card.Content><p class="text-sm">248 records · CSV · all visible fields</p></Card.Content></Card.Root></section>
	<PaginationLoadMore loadedCount={loaded} hasNextPage={loaded < 60} onLoadMore={() => loaded += 20} label="records" />
</div>
