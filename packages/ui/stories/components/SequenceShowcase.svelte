<script lang="ts">
	import * as Stepper from '../../src/primitives/stepper';
	import * as Pagination from '../../src/primitives/pagination';
	import * as Terminal from '../../src/primitives/terminal';
	let step = $state(1);
	let page = $state(1);
</script>
<div class="story-sheet">
	<header><h1>Sequences and progress</h1><p>Use a stepper for a short ordered task, pagination for a known collection, and a terminal only for sequential diagnostic output.</p></header>
	<section><h2>Ordered task</h2><Stepper.Root bind:step><Stepper.Nav>{#each ['Choose records', 'Review options', 'Export'] as label, index (label)}<Stepper.Item><Stepper.Trigger><Stepper.Indicator>{index + 1}</Stepper.Indicator><Stepper.Title>{label}</Stepper.Title></Stepper.Trigger><Stepper.Separator /></Stepper.Item>{/each}</Stepper.Nav><p class="text-sm">Step {step} of 3</p><div class="flex gap-2"><Stepper.Previous>Previous</Stepper.Previous><Stepper.Next>Next</Stepper.Next></div></Stepper.Root></section>
	<section><h2>Known collection</h2><Pagination.Root count={120} perPage={20} bind:page>{#snippet children({pages, currentPage})}<Pagination.Content><Pagination.Item><Pagination.Previous /></Pagination.Item>{#each pages as item (item.key)}{#if item.type === 'ellipsis'}<Pagination.Ellipsis />{:else}<Pagination.Item><Pagination.Link page={item} isActive={currentPage === item.value}>{item.value}</Pagination.Link></Pagination.Item>{/if}{/each}<Pagination.Item><Pagination.Next /></Pagination.Item></Pagination.Content>{/snippet}</Pagination.Root></section>
	<section><h2>Diagnostic output</h2><Terminal.Root><Terminal.AnimatedSpan><span>Prepared 120 records.</span></Terminal.AnimatedSpan><Terminal.AnimatedSpan><span>Export complete. No missing identifiers.</span></Terminal.AnimatedSpan></Terminal.Root></section>
</div>
