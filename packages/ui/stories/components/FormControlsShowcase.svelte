<script lang="ts">
	import * as Field from '../../src/primitives/field';
	import * as Select from '../../src/primitives/select';
	import { Input } from '../../src/primitives/input';
	import { Textarea } from '../../src/primitives/textarea';
	import { Checkbox } from '../../src/primitives/checkbox';
	import { Switch } from '../../src/primitives/switch';
	import { Button } from '../../src/primitives/button';
	let { disabled = false, invalid = false } = $props<{ disabled?: boolean; invalid?: boolean }>();
	let name = $state('Quarterly evidence summary');
	let format = $state('csv');
	let checked = $state(true);
</script>
<div class="story-sheet">
	<header><h1>Form controls</h1><p>Labels explain the value. Help and errors are associated with their control; fields remain readable at narrow widths.</p></header>
	<form onsubmit={(event) => event.preventDefault()} class="max-w-xl">
		<Field.Group>
			<Field.Field data-invalid={invalid} data-disabled={disabled}>
				<Field.Label for="record-name">Record name</Field.Label>
				<Input id="record-name" bind:value={name} {disabled} aria-invalid={invalid} aria-describedby={invalid ? 'name-error' : 'name-help'} />
				{#if invalid}<Field.Error id="name-error">This name is already in use. Choose a distinct name so colleagues can identify the correct version.</Field.Error>{:else}<Field.Description id="name-help">Use a name that distinguishes this version from previous exports.</Field.Description>{/if}
			</Field.Field>
			<Field.Field data-disabled={disabled}>
				<Field.Label for="export-format">Export format</Field.Label>
				<Select.Root type="single" bind:value={format} {disabled}><Select.Trigger id="export-format" class="w-full">{format === 'csv' ? 'CSV — compatible with spreadsheet applications' : 'JSON — structured records'}</Select.Trigger><Select.Content><Select.Group><Select.Item value="csv">CSV — compatible with spreadsheet applications</Select.Item><Select.Item value="json">JSON — structured records</Select.Item></Select.Group></Select.Content></Select.Root>
			</Field.Field>
			<Field.Field><Field.Label for="notes">Notes <span class="text-muted-foreground">(optional)</span></Field.Label><Textarea id="notes" {disabled} placeholder="Describe any limitations or changes in this version." /></Field.Field>
			<Field.Field orientation="horizontal"><Checkbox id="metadata" bind:checked {disabled} /><Field.Label for="metadata">Include metadata and a complete history of changes</Field.Label></Field.Field>
			<Field.Field orientation="horizontal"><Switch id="notifications" {disabled} /><Field.Label for="notifications">Notify me when the export is ready</Field.Label></Field.Field>
			<div class="flex flex-wrap gap-2"><Button type="submit" {disabled}>Create export</Button><Button variant="outline" {disabled}>Cancel</Button></div>
		</Field.Group>
	</form>
</div>
