<script lang="ts">
	import { tick } from 'svelte';
	import { Button } from '@deepref/ui/button';
	import * as DropdownMenu from '@deepref/ui/dropdown-menu';
	import { Input } from '@deepref/ui/input';
	import { Label } from '@deepref/ui/label';
	import { Switch } from '@deepref/ui/switch';
	import { Textarea } from '@deepref/ui/textarea';
	import type { ConfigFieldDto, FieldDetailsDto } from '$lib/api/generated/models';
	import { detailToken, insertDetail } from '../model';
	import ConditionField from './ConditionField.svelte';
	import FieldListField from './FieldListField.svelte';
	import OptionSelect from './OptionSelect.svelte';
	import PublicationQueryField from './PublicationQueryField.svelte';
	import ScheduleField from './ScheduleField.svelte';

	const SECRET_PLACEHOLDER = '__set__';

	let {
		field,
		value,
		extractionFields = [],
		details,
		onchange
	}: {
		field: ConfigFieldDto;
		value: unknown;
		extractionFields?: { id: string; label: string }[];
		/** What this field can read from the steps before it, when the block reads any. */
		details?: FieldDetailsDto;
		onchange: (value: unknown) => void;
	} = $props();

	const inputId = $derived(`cfg-${field.key}`);
	const text = $derived(typeof value === 'string' ? value : value == null ? '' : String(value));
	const duration = $derived(
		typeof value === 'object' && value !== null
			? (value as { value?: number; unit?: string })
			: { value: 5, unit: 'minutes' }
	);
	const UNITS = [
		{ value: 'seconds', label: 'seconds' },
		{ value: 'minutes', label: 'minutes' },
		{ value: 'hours', label: 'hours' },
		{ value: 'days', label: 'days' }
	];

	let inputRef = $state<HTMLInputElement | null>(null);
	let textareaRef = $state<HTMLTextAreaElement | null>(null);

	/** Only text fields that the block fills from earlier steps get the picker. */
	const takesDetails = $derived(
		(field.kind === 'text' || field.kind === 'long_text') &&
			details !== undefined &&
			(details.fields.length > 0 || details.open || details.list)
	);

	/** Put the detail's `{{name}}` where the caret is, then leave the caret after it. */
	async function insert(key: string) {
		const editor = field.kind === 'long_text' ? textareaRef : inputRef;
		const next = insertDetail(
			text,
			key,
			editor?.selectionStart ?? null,
			editor?.selectionEnd ?? null
		);
		onchange(next.text);
		await tick();
		editor?.setSelectionRange(next.caret, next.caret);
		editor?.focus();
	}
</script>

<div class="flex flex-col gap-1.5" data-testid={`field-${field.key}`}>
	{#if field.kind !== 'boolean' && field.kind !== 'publication_query'}
		<div class="flex min-h-7 items-center justify-between gap-2">
			<Label for={inputId}>
				{field.label}{#if field.required}<span class="text-destructive" aria-hidden="true">
						*</span
					>{/if}
			</Label>
			{#if takesDetails && details}
				<DropdownMenu.Root>
					<DropdownMenu.Trigger>
						{#snippet child({ props })}
							<Button {...props} type="button" variant="ghost" size="xs">
								Insert value
							</Button>
						{/snippet}
					</DropdownMenu.Trigger>
					<DropdownMenu.Content align="end" class="max-h-80 w-72 overflow-y-auto">
						<DropdownMenu.Label>
							From {details.source ?? 'the step before'}
						</DropdownMenu.Label>
						{#each details.fields as detail (detail.key)}
							<DropdownMenu.Item onclick={() => insert(detail.key)}>
								<span class="flex-1 truncate">{detail.label}</span>
								<code class="shrink-0 text-xs text-muted-foreground">
									{detailToken(detail.key)}
								</code>
							</DropdownMenu.Item>
						{/each}
						{#if details.list}
							<p class="px-2 py-1.5 text-xs text-muted-foreground">
								It is a list. Use the count, or add “Do this for each record” before
								this step to use each record's details.
							</p>
						{/if}
						{#if details.open}
							<p class="px-2 py-1.5 text-xs text-muted-foreground">
								Its names depend on what is sent. Type a name between double braces.
							</p>
						{/if}
					</DropdownMenu.Content>
				</DropdownMenu.Root>
			{/if}
		</div>
	{/if}

	{#if field.kind === 'text'}
		<Input
			id={inputId}
			bind:ref={inputRef}
			placeholder={field.placeholder ?? ''}
			value={text}
			oninput={(event) => onchange(event.currentTarget.value)}
		/>
	{:else if field.kind === 'long_text'}
		<Textarea
			id={inputId}
			bind:ref={textareaRef}
			placeholder={field.placeholder ?? ''}
			value={text}
			oninput={(event) => onchange(event.currentTarget.value)}
		/>
	{:else if field.kind === 'number'}
		<Input
			id={inputId}
			type="number"
			min={field.min ?? undefined}
			max={field.max ?? undefined}
			value={text}
			oninput={(event) => {
				const raw = event.currentTarget.value;
				onchange(raw === '' ? null : Number(raw));
			}}
		/>
	{:else if field.kind === 'select'}
		<OptionSelect
			label={field.label}
			value={text}
			options={field.options}
			onchange={(next) => onchange(next)}
		/>
	{:else if field.kind === 'boolean'}
		<div class="flex items-center justify-between gap-3">
			<Label for={inputId}>{field.label}</Label>
			<Switch
				id={inputId}
				checked={value === true}
				onCheckedChange={(next) => onchange(next)}
			/>
		</div>
	{:else if field.kind === 'project_field'}
		<OptionSelect
			label={field.label}
			value={text}
			placeholder={extractionFields.length ? 'Choose a field…' : 'No extraction fields yet'}
			options={extractionFields.map((item) => ({ value: item.id, label: item.label }))}
			onchange={(next) => onchange(next)}
		/>
	{:else if field.kind === 'secret'}
		<Input
			id={inputId}
			type="password"
			autocomplete="off"
			placeholder={text === SECRET_PLACEHOLDER
				? 'Saved — type to replace'
				: (field.placeholder ?? '')}
			value={text === SECRET_PLACEHOLDER ? '' : text}
			oninput={(event) =>
				onchange(
					event.currentTarget.value ||
						(text === SECRET_PLACEHOLDER ? SECRET_PLACEHOLDER : '')
				)}
		/>
	{:else if field.kind === 'duration'}
		<div class="flex gap-2">
			<Input
				id={inputId}
				type="number"
				min={1}
				class="w-24"
				value={duration.value ?? 5}
				oninput={(event) =>
					onchange({ ...duration, value: Number(event.currentTarget.value) || 1 })}
			/>
			<OptionSelect
				label="Unit"
				value={duration.unit ?? 'minutes'}
				options={UNITS}
				onchange={(unit) => onchange({ ...duration, unit })}
			/>
		</div>
	{:else if field.kind === 'schedule'}
		<ScheduleField {value} {onchange} />
	{:else if field.kind === 'condition'}
		<ConditionField {value} {onchange} {details} />
	{:else if field.kind === 'publication_query'}
		<PublicationQueryField {value} {onchange} />
	{:else if field.kind === 'field_list'}
		<FieldListField {value} {onchange} />
	{:else}
		<Input id={inputId} value={text} oninput={(event) => onchange(event.currentTarget.value)} />
	{/if}

	{#if field.help && field.kind !== 'publication_query'}
		<p class="text-xs text-muted-foreground">{field.help}</p>
	{/if}
	{#if takesDetails}
		<p class="text-xs text-muted-foreground">
			Write a name between double braces, for example {'{{title}}'}, or pick one with Insert
			value.
		</p>
	{/if}
</div>
