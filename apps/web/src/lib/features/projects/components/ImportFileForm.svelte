<script lang="ts">
	import * as Alert from '@deepref/ui/alert';
	import * as Field from '@deepref/ui/field';
	import { Button } from '@deepref/ui/button';
	import { Input } from '@deepref/ui/input';
	import * as Select from '@deepref/ui/select';
	import * as Table from '@deepref/ui/table';
	import { ApiError } from '#lib/api/custom-fetch.js';
	import {
		getListAcquisitionsQueryKey,
		importProjectRecords
	} from '#lib/api/generated/acquisitions/acquisitions.js';
	import { getGetProjectPrismaQueryKey } from '#lib/api/generated/review/review.js';
	import type { ImportRecords } from '#lib/api/generated/models/index.js';
	import { useQueryClient } from '@tanstack/svelte-query';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import CircleCheckIcon from '@lucide/svelte/icons/circle-check';
	import UploadIcon from '@lucide/svelte/icons/upload';
	import { useProjectWorkspaceContext } from '../context.svelte.js';
	import {
		CSV_MAPPING_FIELDS,
		CSV_MAPPING_LABEL,
		IMPORT_FILE_FORMAT_OPTIONS,
		IMPORT_RUN_FORMAT_LABEL,
		MAX_IMPORT_FILE_BYTES,
		buildCsvMapping,
		csvMappingProblems,
		detectImportFormat,
		formatImportError,
		guessCsvColumns,
		parseCsvHeaders,
		parseCsvRecords,
		pluralize,
		type CsvColumnSelection,
		type ImportFileFormat,
		type ImportFormatChoice
	} from '../imports';

	let {
		onDoiList
	}: {
		/** A plain-text file of DOIs belongs in the DOI form, which fetches and follows them. */
		onDoiList: (text: string, fileName: string) => void;
	} = $props();

	const workspace = useProjectWorkspaceContext();
	const queryClient = useQueryClient();
	const projectId = $derived(workspace.project.id);
	const PREVIEW_ROWS = 3;
	const NO_COLUMN = '__no_column__';

	let fileName = $state<string | undefined>();
	let content = $state<string | undefined>();
	let readError = $state<string | undefined>();
	let detected = $state<ImportFormatChoice | undefined>();
	let format = $state<ImportFileFormat | undefined>();
	let csvSelection = $state<CsvColumnSelection>({});
	let submitting = $state(false);
	let submitError = $state<string | undefined>();
	let imported = $state<{ created: boolean; records: number; label: string } | undefined>();

	const csvRecords = $derived(
		content !== undefined && format === 'csv' ? parseCsvRecords(content) : []
	);
	const csvHeaders = $derived(csvRecords[0] ?? []);
	const csvPreview = $derived(csvRecords.slice(1, 1 + PREVIEW_ROWS));
	const mappedFields = $derived(
		CSV_MAPPING_FIELDS.filter((field) => Boolean(csvSelection[field]))
	);
	const mappingProblems = $derived(format === 'csv' ? csvMappingProblems(csvSelection) : []);
	const canImport = $derived(
		content !== undefined &&
			format !== undefined &&
			mappingProblems.length === 0 &&
			!submitting &&
			readError === undefined
	);
	const formatLabel = $derived(format ? IMPORT_RUN_FORMAT_LABEL[format] : undefined);

	function reset() {
		fileName = undefined;
		content = undefined;
		readError = undefined;
		detected = undefined;
		format = undefined;
		csvSelection = {};
		submitError = undefined;
		imported = undefined;
	}

	function chooseFormat(next: ImportFileFormat | undefined) {
		format = next;
		submitError = undefined;
		imported = undefined;
		if (next === 'csv') csvSelection = guessCsvColumns(parseCsvHeaders(content ?? ''));
	}

	async function chooseFile(event: Event) {
		const input = event.currentTarget as HTMLInputElement;
		const file = input.files?.[0];
		reset();
		if (!file) return;
		fileName = file.name;
		if (file.size > MAX_IMPORT_FILE_BYTES) {
			readError = `${file.name} is ${(file.size / 1024 / 1024).toFixed(1)} MB. Imports are limited to 2 MB, so split the export and import the parts.`;
			return;
		}
		try {
			const text = await file.text();
			const choice = detectImportFormat(file.name, text);
			if (choice === 'doi') {
				onDoiList(text, file.name);
				input.value = '';
				reset();
				return;
			}
			content = text;
			detected = choice;
			chooseFormat(choice);
		} catch {
			readError = `${file.name} could not be read. Save it again and retry.`;
		}
	}

	async function importIdempotencyKey(body: ImportRecords): Promise<string> {
		// The key is derived from what is imported, so repeating the same import returns the
		// existing run instead of adding the records a second time.
		const material = new TextEncoder().encode(
			JSON.stringify([body.format, body.csv_mapping ?? null, body.content])
		);
		try {
			const digest = await globalThis.crypto.subtle.digest('SHA-256', material);
			const hex = Array.from(new Uint8Array(digest), (byte) =>
				byte.toString(16).padStart(2, '0')
			);
			return `file-import:${hex.join('')}`;
		} catch {
			const random = globalThis.crypto?.randomUUID?.();
			if (!random) throw new Error('This browser cannot create a secure import key.');
			return `file-import:${random}`;
		}
	}

	function importErrorMessage(error: unknown): string {
		if (error instanceof ApiError) return formatImportError(error.message);
		return 'The import could not be sent. Check your connection and try again.';
	}

	async function submit() {
		if (content === undefined || format === undefined) return;
		const problems = mappingProblems;
		if (problems.length > 0) {
			submitError = problems[0];
			return;
		}
		submitting = true;
		submitError = undefined;
		imported = undefined;
		const label = IMPORT_RUN_FORMAT_LABEL[format];
		const body: ImportRecords = {
			format,
			content,
			csv_mapping: format === 'csv' ? buildCsvMapping(csvSelection) : undefined
		};
		try {
			const key = await importIdempotencyKey(body);
			const response = await importProjectRecords(projectId, body, {
				headers: { 'Idempotency-Key': key }
			});
			imported = {
				created: response.status === 201,
				records: response.data.queued_count,
				label
			};
			await Promise.all([
				queryClient.invalidateQueries({ queryKey: getGetProjectPrismaQueryKey(projectId) }),
				queryClient.invalidateQueries({
					queryKey: getListAcquisitionsQueryKey(projectId, { strategy: 'file_import' })
				})
			]);
		} catch (error) {
			submitError = importErrorMessage(error);
		} finally {
			submitting = false;
		}
	}

	function columnLabel(column: string): string {
		return column.trim() === '' ? '(blank header)' : column.trim();
	}

	function triggerLabel(value: string | undefined): string {
		return value ? columnLabel(value) : 'Not in this file';
	}
</script>

<form
	class="flex max-w-3xl flex-col gap-5"
	data-testid="file-import-form"
	onsubmit={(event) => {
		event.preventDefault();
		void submit();
	}}
>
	<Field.Field>
		<Field.FieldLabel for="import-file">Export file</Field.FieldLabel>
		<Input
			id="import-file"
			type="file"
			accept=".ris,.nbib,.bib,.bibtex,.csv,.txt"
			class="cursor-pointer file:mr-3"
			onchange={chooseFile}
			data-testid="import-file-input"
		/>
		<Field.FieldDescription>
			RIS, NBIB (PubMed MEDLINE), BibTeX or CSV, up to 2 MB. A plain-text list of DOIs goes to
			the DOI form.
		</Field.FieldDescription>
	</Field.Field>

	{#if readError}
		<Alert.Root variant="destructive" role="alert">
			<CircleAlertIcon />
			<Alert.Description>{readError}</Alert.Description>
		</Alert.Root>
	{/if}

	{#if content !== undefined && fileName}
		<div class="flex flex-col gap-4 rounded-lg border bg-card p-4">
			<p class="flex items-center gap-2 text-sm">
				<UploadIcon class="size-4 text-muted-foreground" aria-hidden="true" />
				<span class="min-w-0 truncate font-medium" title={fileName}>{fileName}</span>
				{#if detected}
					<span class="text-muted-foreground"
						>· detected as {IMPORT_RUN_FORMAT_LABEL[detected]}</span
					>
				{/if}
			</p>

			<Field.Field>
				<Field.FieldLabel for="import-format">File format</Field.FieldLabel>
				<Select.Root
					type="single"
					value={format ?? ''}
					onValueChange={(next) => {
						if (next) chooseFormat(next as ImportFileFormat);
					}}
				>
					<Select.Trigger
						id="import-format"
						class="w-full max-w-sm"
						aria-label="File format"
					>
						{format
							? IMPORT_FILE_FORMAT_OPTIONS.find((option) => option.value === format)
									?.label
							: 'Choose a format'}
					</Select.Trigger>
					<Select.Content>
						<Select.Group>
							{#each IMPORT_FILE_FORMAT_OPTIONS as option (option.value)}
								<Select.Item value={option.value} label={option.label} />
							{/each}
						</Select.Group>
					</Select.Content>
				</Select.Root>
				{#if !format}
					<Field.FieldDescription>
						DeepRef could not tell the format from this file. Choose it above.
					</Field.FieldDescription>
				{/if}
			</Field.Field>

			{#if format === 'csv'}
				<section
					aria-label="Column mapping"
					class="flex flex-col gap-3"
					data-testid="csv-mapping"
				>
					<div>
						<h3 class="text-sm font-semibold">Map the columns</h3>
						<p class="text-xs text-muted-foreground">
							Choose which column holds each field. Columns that aren't listed are
							kept as they are in the file.
						</p>
					</div>
					<div class="grid grid-cols-1 gap-3 sm:grid-cols-2">
						{#each CSV_MAPPING_FIELDS as field (field)}
							<Field.Field>
								<Field.FieldLabel for={`map-${field}`}
									>{CSV_MAPPING_LABEL[field]}</Field.FieldLabel
								>
								<Select.Root
									type="single"
									value={csvSelection[field] ?? NO_COLUMN}
									onValueChange={(next) => {
										csvSelection = {
											...csvSelection,
											[field]: next && next !== NO_COLUMN ? next : undefined
										};
									}}
								>
									<Select.Trigger
										id={`map-${field}`}
										class="w-full"
										aria-label={`Column for ${CSV_MAPPING_LABEL[field]}`}
									>
										{triggerLabel(csvSelection[field])}
									</Select.Trigger>
									<Select.Content>
										<Select.Group>
											<Select.Item
												value={NO_COLUMN}
												label="Not in this file"
											/>
											{#each csvHeaders as header, index (`${index}-${header}`)}
												{#if header !== ''}
													<Select.Item
														value={header}
														label={columnLabel(header)}
													/>
												{/if}
											{/each}
										</Select.Group>
									</Select.Content>
								</Select.Root>
							</Field.Field>
						{/each}
					</div>
					{#if mappingProblems.length > 0}
						<p class="text-xs text-warning" role="status">{mappingProblems[0]}</p>
					{/if}
					{#if mappedFields.length > 0 && csvPreview.length > 0}
						<div class="max-w-full overflow-x-auto rounded-md border">
							<Table.Root containerLabel="Preview of mapped rows">
								<Table.Header>
									<Table.Row>
										{#each mappedFields as field (field)}
											<Table.Head>{CSV_MAPPING_LABEL[field]}</Table.Head>
										{/each}
									</Table.Row>
								</Table.Header>
								<Table.Body>
									{#each csvPreview as record, rowIndex (rowIndex)}
										<Table.Row>
											{#each mappedFields as field (field)}
												{@const column = csvHeaders.indexOf(
													csvSelection[field] ?? ''
												)}
												<Table.Cell
													class="max-w-48 truncate whitespace-nowrap"
												>
													{record[column] ?? ''}
												</Table.Cell>
											{/each}
										</Table.Row>
									{/each}
								</Table.Body>
							</Table.Root>
						</div>
						<p class="text-xs text-muted-foreground tabular-nums">
							{pluralize(csvRecords.length - 1, 'data row')} in this file.
						</p>
					{/if}
				</section>
			{/if}

			{#if submitError}
				<Alert.Root variant="destructive" role="alert" data-testid="file-import-error">
					<CircleAlertIcon />
					<Alert.Title>Nothing was imported</Alert.Title>
					<Alert.Description>{submitError}</Alert.Description>
				</Alert.Root>
			{/if}

			<div class="flex flex-wrap items-center justify-between gap-3">
				<p class="text-xs text-muted-foreground">
					Duplicates are checked after the import, on the Deduplication page.
				</p>
				<Button type="submit" disabled={!canImport}>
					<UploadIcon data-icon="inline-start" />{submitting
						? 'Importing…'
						: formatLabel
							? `Import ${formatLabel}`
							: 'Import file'}
				</Button>
			</div>
		</div>
	{/if}

	{#if imported}
		<Alert.Root role="status" data-testid="file-import-result">
			<CircleCheckIcon />
			<Alert.Title>
				{imported.created ? 'Import complete' : 'Already imported'}
			</Alert.Title>
			<Alert.Description>
				{imported.created
					? `${pluralize(imported.records, 'record')} from the ${imported.label} added to this project.`
					: `This file was imported before. Nothing new was added; the run is listed below.`}
			</Alert.Description>
		</Alert.Root>
	{/if}
</form>
