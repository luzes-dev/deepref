import { statusVariant } from '$lib/api/helpers';

/** File formats the file import accepts; mirrors `parse_format` in the acquisitions route. */
export type ImportFileFormat = 'ris' | 'nbib' | 'bibtex' | 'csv';

/** What a file looks like: a file format, or a plain list of DOIs (routed to the DOI form). */
export type ImportFormatChoice = ImportFileFormat | 'doi';

export const IMPORT_FILE_FORMAT_OPTIONS: ReadonlyArray<{
	value: ImportFileFormat;
	label: string;
}> = [
	{ value: 'ris', label: 'RIS' },
	{ value: 'nbib', label: 'NBIB (PubMed MEDLINE)' },
	{ value: 'bibtex', label: 'BibTeX' },
	{ value: 'csv', label: 'CSV with column mapping' }
];

/** Matches the 2 MiB request limit enforced by the import route. */
export const MAX_IMPORT_FILE_BYTES = 2 * 1024 * 1024;

export const IMPORT_RUN_FORMAT_LABEL: Record<ImportFormatChoice, string> = {
	ris: 'RIS file',
	nbib: 'NBIB file',
	bibtex: 'BibTeX file',
	csv: 'CSV file',
	doi: 'DOI list'
};

export type LineIssue = {
	/** 1-based line number in the text the user pasted or the file they chose. */
	line: number;
	value: string;
	message: string;
};

export type DoiListParse = {
	/** Unique normalised DOIs, in first-seen order. */
	dois: string[];
	/** Valid DOIs that repeated an earlier one. */
	duplicates: number;
	issues: LineIssue[];
};

export type PmidListParse = {
	pmids: string[];
	duplicates: number;
	issues: LineIssue[];
};

const DOI_PREFIX = /^(?:(?:https?:\/\/)?(?:dx\.|www\.)?doi\.org\/|doi:)/;
// Same shape rule as the server (`normalize_doi`): a 10. prefix and a slash.
const DOI_SHAPE = /^10\.[^\s/]+\/\S+$/;

/**
 * Normalises one DOI-like token: lower case, no NFKC surprises, no `doi:` or doi.org prefix,
 * no trailing full stops. Returns `undefined` when the result is not a DOI.
 */
export function normalizeDoi(input: string): string | undefined {
	const value = input
		.normalize('NFKC')
		.trim()
		.toLowerCase()
		.replace(DOI_PREFIX, '')
		.trim()
		.replace(/^\.+|\.+$/g, '');
	return DOI_SHAPE.test(value) ? value : undefined;
}

/**
 * Parses pasted DOIs. Lines may hold several DOIs separated by commas or spaces, since a DOI
 * never contains whitespace. Every token that is not a DOI is reported with its line number.
 */
export function parseDoiList(text: string): DoiListParse {
	const dois: string[] = [];
	const seen = new Set<string>();
	const issues: LineIssue[] = [];
	let duplicates = 0;

	text.split(/\r?\n/).forEach((rawLine, index) => {
		if (/^\s*pmid\b/i.test(rawLine)) {
			// A PubMed ID line is one problem, not one per word of "PMID: 123".
			issues.push({
				line: index + 1,
				value: rawLine.trim(),
				message: 'is a PubMed ID; add it on the PMIDs tab'
			});
			return;
		}
		const line = rawLine.replace(/\bdoi:\s+/gi, 'doi:');
		for (const token of line.split(/[\s,]+/)) {
			if (!token) continue;
			const doi = normalizeDoi(token);
			if (doi === undefined) {
				issues.push({ line: index + 1, value: token, message: 'is not a DOI' });
			} else if (seen.has(doi)) {
				duplicates += 1;
			} else {
				seen.add(doi);
				dois.push(doi);
			}
		}
	});

	return { dois, duplicates, issues };
}

/**
 * Normalises one PubMed ID: drops a `PMID:` label or a pubmed.ncbi.nlm.nih.gov link, and
 * returns the digits, or `undefined` when what remains is not a PMID.
 */
export function normalizePmid(input: string): string | undefined {
	const value = input
		.trim()
		.replace(/^(?:(?:https?:\/\/)?pubmed\.ncbi\.nlm\.nih\.gov\/|pmid\s*:?\s*#?\s*)/i, '')
		.replace(/\/+$/, '')
		.trim();
	return /^\d{1,9}$/.test(value) ? value : undefined;
}

/** Parses pasted PubMed IDs, one per line; commas also separate them. */
export function parsePmidList(text: string): PmidListParse {
	const pmids: string[] = [];
	const seen = new Set<string>();
	const issues: LineIssue[] = [];
	let duplicates = 0;

	text.split(/\r?\n/).forEach((rawLine, index) => {
		for (const token of rawLine.split(',')) {
			const trimmed = token.trim();
			if (!trimmed) continue;
			const pmid = normalizePmid(trimmed);
			if (pmid === undefined) {
				issues.push({ line: index + 1, value: trimmed, message: 'is not a PubMed ID' });
			} else if (seen.has(pmid)) {
				duplicates += 1;
			} else {
				seen.add(pmid);
				pmids.push(pmid);
			}
		}
	});

	return { pmids, duplicates, issues };
}

/** Works out the import format from the file name, then from the first part of its text. */
export function detectImportFormat(
	fileName: string,
	sample: string
): ImportFormatChoice | undefined {
	const extension = fileName.includes('.') ? (fileName.split('.').pop() ?? '') : '';
	switch (extension.toLowerCase()) {
		case 'ris':
			return 'ris';
		case 'nbib':
			return 'nbib';
		case 'bib':
		case 'bibtex':
		case 'biblatex':
			return 'bibtex';
		case 'csv':
			return 'csv';
		default:
			return sniffImportFormat(sample);
	}
}

/** Recognises RIS, NBIB and BibTeX by their first tags. Plain text is only a DOI list if every token is a DOI. */
export function sniffImportFormat(sample: string): ImportFormatChoice | undefined {
	const head = sample.slice(0, 20_000);
	if (/^TY {2}- /m.test(head)) return 'ris';
	if (/^PMID- /m.test(head) || /^OWN {2}- /m.test(head)) return 'nbib';
	if (/^\s*@[a-z]+\s*\{/im.test(head)) return 'bibtex';
	const doiList = parseDoiList(sample);
	if (doiList.dois.length > 0 && doiList.issues.length === 0) return 'doi';
	return undefined;
}

/**
 * Splits CSV text into records. Handles quoted fields with embedded commas, line breaks and
 * doubled quotes, CRLF line endings, and a leading byte-order mark. Blank lines are dropped.
 */
export function parseCsvRecords(text: string): string[][] {
	const input = text.replace(/^\uFEFF/, '');
	const rows: string[][] = [];
	let row: string[] = [];
	let field = '';
	let quoted = false;

	for (let index = 0; index < input.length; index += 1) {
		const char = input[index];
		if (quoted) {
			if (char === '"' && input[index + 1] === '"') {
				field += '"';
				index += 1;
			} else if (char === '"') {
				quoted = false;
			} else {
				field += char;
			}
		} else if (char === '"') {
			quoted = true;
		} else if (char === ',') {
			row.push(field);
			field = '';
		} else if (char === '\n' || char === '\r') {
			if (char === '\r' && input[index + 1] === '\n') index += 1;
			row.push(field);
			rows.push(row);
			row = [];
			field = '';
		} else {
			field += char;
		}
	}
	if (field !== '' || row.length > 0) {
		row.push(field);
		rows.push(row);
	}
	return rows.filter((record) => !(record.length === 1 && record[0] === ''));
}

/** Column names exactly as the CSV header spells them; the server matches them byte for byte. */
export function parseCsvHeaders(text: string): string[] {
	return parseCsvRecords(text)[0] ?? [];
}

export const CSV_MAPPING_FIELDS = [
	'title',
	'abstract_text',
	'authors',
	'publication_year',
	'journal',
	'doi',
	'pmid',
	'pmcid'
] as const;

export type CsvMappingField = (typeof CSV_MAPPING_FIELDS)[number];

export const CSV_MAPPING_LABEL: Record<CsvMappingField, string> = {
	title: 'Title',
	abstract_text: 'Abstract',
	authors: 'Authors',
	publication_year: 'Year',
	journal: 'Journal',
	doi: 'DOI',
	pmid: 'PMID',
	pmcid: 'PMCID'
};

/** Which CSV column holds each field. A field that is absent or empty means "not in this file". */
export type CsvColumnSelection = Partial<Record<CsvMappingField, string>>;

const CSV_GUESSES: Record<CsvMappingField, readonly string[]> = {
	title: ['title', 'articletitle', 'ti', 'papertitle'],
	abstract_text: ['abstract', 'ab', 'abstracttext', 'summary'],
	authors: ['authors', 'author', 'au', 'authorlist', 'authornames'],
	publication_year: ['year', 'publicationyear', 'pubyear', 'py', 'yearpublished', 'date'],
	journal: ['journal', 'journaltitle', 'sourcetitle', 'containertitle', 'publication', 'venue'],
	doi: ['doi', 'doiurl', 'digitalobjectidentifier'],
	pmid: ['pmid', 'pubmedid', 'pubmed'],
	pmcid: ['pmcid', 'pmc']
};

function headerKey(header: string): string {
	return header.toLowerCase().replace(/[^a-z0-9]/g, '');
}

/** Proposes a column for each field by matching header names. Each column is used at most once. */
export function guessCsvColumns(headers: readonly string[]): CsvColumnSelection {
	const selection: CsvColumnSelection = {};
	const taken = new Set<string>();
	for (const field of CSV_MAPPING_FIELDS) {
		const match = CSV_GUESSES[field]
			.map((guess) => headers.find((header) => headerKey(header) === guess))
			.find((header) => header !== undefined && !taken.has(header));
		if (match !== undefined) {
			selection[field] = match;
			taken.add(match);
		}
	}
	return selection;
}

/** The `csv_mapping` body the server expects: only the fields that have a column. */
export function buildCsvMapping(
	selection: CsvColumnSelection
): Partial<Record<CsvMappingField, string>> {
	const mapping: Partial<Record<CsvMappingField, string>> = {};
	for (const field of CSV_MAPPING_FIELDS) {
		const column = selection[field];
		if (column) mapping[field] = column;
	}
	return mapping;
}

/** Reasons the mapping cannot be imported yet. An empty list means the mapping is usable. */
export function csvMappingProblems(selection: CsvColumnSelection): string[] {
	const mapping = buildCsvMapping(selection);
	if (Object.keys(mapping).length === 0) return ['Choose at least one column.'];
	if (!mapping.title && !mapping.doi) {
		return ['Map a Title or DOI column, so each record can be identified.'];
	}
	return [];
}

/**
 * Turns a server import error into a sentence that names the line. The server reports
 * "invalid ris import: invalid tagged field at line 7"; the user sees "Line 7: invalid tagged field."
 */
export function formatImportError(message: string): string {
	const match = /^invalid [a-z]+ import: (.+) at line (\d+)$/i.exec(message.trim());
	if (match) {
		const [, detail, line] = match;
		return `Line ${line}: ${detail.charAt(0).toUpperCase()}${detail.slice(1)}.`;
	}
	const bare = /^invalid [a-z]+ import: (.+)$/i.exec(message.trim());
	if (bare) return bare[1].charAt(0).toUpperCase() + bare[1].slice(1);
	return message;
}

/** Label for a run in the runs table, for example "RIS file". */
export function importRunFormatLabel(format: string | null | undefined): string {
	if (format === 'ris' || format === 'nbib' || format === 'bibtex' || format === 'csv') {
		return IMPORT_RUN_FORMAT_LABEL[format];
	}
	if (format === 'doi') return IMPORT_RUN_FORMAT_LABEL.doi;
	if (format === 'pmid') return 'PubMed IDs';
	return 'Import';
}

export type RunStatusDisplay = {
	label: string;
	variant: 'default' | 'secondary' | 'destructive' | 'outline' | 'warning';
};

/**
 * How a run's status reads in the UI. A run that finished but missed some articles is
 * "completed with problems" (amber), not failed. Only a run that fetched nothing is failed.
 */
export function runStatusDisplay(status: string, failedCount: number): RunStatusDisplay {
	if (status === 'completed' && failedCount > 0) {
		return { label: 'completed with problems', variant: 'warning' };
	}
	return { label: status.replaceAll('_', ' '), variant: statusVariant(status) };
}

/** Plain-language label for one article inside a run. */
export function ingestionItemStatusLabel(status: string): string {
	switch (status) {
		case 'not_found':
			return 'Not found in Crossref';
		case 'failed':
			return 'Could not be fetched';
		case 'fetched':
			return 'Fetched';
		case 'queued':
			return 'Queued';
		case 'fetching':
			return 'Fetching';
		default:
			return status.replaceAll('_', ' ');
	}
}

/** Badge variant for one article inside a run. Missing from Crossref is a warning, not an error. */
export function ingestionItemStatusVariant(status: string): RunStatusDisplay['variant'] {
	if (status === 'not_found') return 'warning';
	if (status === 'failed') return 'destructive';
	return statusVariant(status);
}

/** Plain-language label for one PubMed ID inside a PubMed ID run. */
export function pmidItemStatusLabel(status: string): string {
	switch (status) {
		case 'imported':
			return 'Imported';
		case 'already_in_project':
			return 'Already in project';
		case 'not_found':
			return 'Not found in PubMed';
		case 'failed':
			return 'Could not be fetched';
		case 'queued':
			return 'Waiting for PubMed';
		default:
			return status.replaceAll('_', ' ');
	}
}

/** Badge variant for one PubMed ID inside a run. Not found and failed are the misses. */
export function pmidItemStatusVariant(status: string): RunStatusDisplay['variant'] {
	switch (status) {
		case 'imported':
			return 'default';
		case 'already_in_project':
			return 'secondary';
		case 'not_found':
			return 'warning';
		case 'failed':
			return 'destructive';
		default:
			return 'outline';
	}
}

export function pluralize(count: number, singular: string, plural = `${singular}s`): string {
	return `${count} ${count === 1 ? singular : plural}`;
}
