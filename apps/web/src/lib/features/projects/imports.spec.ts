import { describe, expect, it } from 'vitest';
import {
	buildCsvMapping,
	csvMappingProblems,
	detectImportFormat,
	formatImportError,
	guessCsvColumns,
	importRunFormatLabel,
	ingestionItemStatusLabel,
	ingestionItemStatusVariant,
	normalizeDoi,
	normalizePmid,
	parseCsvHeaders,
	parseCsvRecords,
	parseDoiList,
	parsePmidList,
	pmidItemStatusLabel,
	pmidItemStatusVariant,
	runStatusDisplay,
	sniffImportFormat
} from './imports';

describe('normalizeDoi', () => {
	it.each([
		['10.1016/S0140-6736(09)60658-9', '10.1016/s0140-6736(09)60658-9'],
		['https://doi.org/10.1016/S0140-6736(09)60658-9', '10.1016/s0140-6736(09)60658-9'],
		['http://dx.doi.org/10.1000/182', '10.1000/182'],
		['doi:10.1000/182', '10.1000/182'],
		['DOI: 10.1000/182', '10.1000/182'],
		['  10.1000/182.  ', '10.1000/182'],
		['www.doi.org/10.1000/182', '10.1000/182'],
		['10.1/new', '10.1/new']
	])('normalises %s to %s', (input, expected) => {
		expect(normalizeDoi(input)).toBe(expected);
	});

	it.each(['not-a-doi', '11.1000/182', '10./x', '10.1000', 'https://example.org/10.1000/1', ''])(
		'rejects %j',
		(input) => {
			expect(normalizeDoi(input)).toBeUndefined();
		}
	);
});

describe('parseDoiList', () => {
	it('counts unique normalised DOIs, so one article written two ways is one seed', () => {
		const parsed = parseDoiList(
			[
				'10.1016/S0140-6736(09)60658-9',
				'https://doi.org/10.1016/s0140-6736(09)60658-9',
				'10.1000/182',
				'10.1056/nejmoa2001017'
			].join('\n')
		);
		expect(parsed.dois).toEqual([
			'10.1016/s0140-6736(09)60658-9',
			'10.1000/182',
			'10.1056/nejmoa2001017'
		]);
		expect(parsed.duplicates).toBe(1);
		expect(parsed.issues).toEqual([]);
	});

	it('names every invalid line by its number', () => {
		const parsed = parseDoiList('10.1000/182\n\nPMID: 19446324\nnot-a-doi');
		expect(parsed.dois).toEqual(['10.1000/182']);
		expect(parsed.issues).toEqual([
			{
				line: 3,
				value: 'PMID: 19446324',
				message: 'is a PubMed ID; add it on the PMIDs tab'
			},
			{ line: 4, value: 'not-a-doi', message: 'is not a DOI' }
		]);
	});

	it('accepts several DOIs on one line separated by commas or spaces', () => {
		const parsed = parseDoiList('10.1000/182, 10.1056/nejmoa2001017 10.1000/183');
		expect(parsed.dois).toEqual(['10.1000/182', '10.1056/nejmoa2001017', '10.1000/183']);
	});

	it('reports a PubMed ID line once, pointing to the PMIDs tab', () => {
		const parsed = parseDoiList('10.1000/182\nPMID: 19446324');
		expect(parsed.issues).toEqual([
			{ line: 2, value: 'PMID: 19446324', message: 'is a PubMed ID; add it on the PMIDs tab' }
		]);
	});

	it('joins a "doi: " label to the DOI that follows it', () => {
		expect(parseDoiList('doi: 10.1000/182').dois).toEqual(['10.1000/182']);
	});
});

describe('PubMed IDs', () => {
	it.each([
		['19446324', '19446324'],
		['PMID: 19446324', '19446324'],
		['pmid 19446324', '19446324'],
		['https://pubmed.ncbi.nlm.nih.gov/19446324/', '19446324']
	])('normalises %s', (input, expected) => {
		expect(normalizePmid(input)).toBe(expected);
	});

	it.each(['10.1000/182', 'PMID: abc', '1234567890', ''])('rejects %j', (input) => {
		expect(normalizePmid(input)).toBeUndefined();
	});

	it('reports the offending line and drops repeats', () => {
		const parsed = parsePmidList('PMID: 19446324\nPMID: x12\n19446324, 123');
		expect(parsed.pmids).toEqual(['19446324', '123']);
		expect(parsed.duplicates).toBe(1);
		expect(parsed.issues).toEqual([
			{ line: 2, value: 'PMID: x12', message: 'is not a PubMed ID' }
		]);
	});
});

describe('detectImportFormat', () => {
	it.each([
		['search-export.ris', 'ris'],
		['Scopus.RIS', 'ris'],
		['pubmed-result.nbib', 'nbib'],
		['refs.bib', 'bibtex'],
		['refs.bibtex', 'bibtex'],
		['studies.csv', 'csv']
	])('detects %s from its extension', (fileName, expected) => {
		expect(detectImportFormat(fileName, '')).toBe(expected);
	});

	it('sniffs text files from their first tags', () => {
		expect(detectImportFormat('export.txt', 'TY  - JOUR\nTI  - A trial\nER  - \n')).toBe('ris');
		expect(detectImportFormat('export.txt', 'PMID- 19446324\nTI  - A trial\n')).toBe('nbib');
		expect(detectImportFormat('export.txt', '@article{smith2020,\n title={A trial}}')).toBe(
			'bibtex'
		);
	});

	it('treats a text file of DOIs as a DOI list', () => {
		expect(
			detectImportFormat('dois.txt', '10.1000/182\nhttps://doi.org/10.1056/nejmoa2001017\n')
		).toBe('doi');
		expect(sniffImportFormat('10.1000/182\nnot-a-doi')).toBeUndefined();
	});

	it('returns undefined when nothing identifies the file', () => {
		expect(detectImportFormat('notes.pdf', 'hello')).toBeUndefined();
		expect(detectImportFormat('README', '')).toBeUndefined();
	});
});

describe('CSV parsing and column mapping', () => {
	it('keeps quoted commas, doubled quotes and line breaks inside one field', () => {
		const records = parseCsvRecords(
			'\uFEFFtitle,authors\r\n"A trial, of ""things""","Smith J; Lee K"\r\n\r\n"two\nlines",X\r\n'
		);
		expect(records).toEqual([
			['title', 'authors'],
			['A trial, of "things"', 'Smith J; Lee K'],
			['two\nlines', 'X']
		]);
	});

	it('reads the header row exactly as written', () => {
		expect(parseCsvHeaders('Title, Year\n')).toEqual(['Title', ' Year']);
	});

	it('guesses columns from common export headers and uses each column once', () => {
		const selection = guessCsvColumns([
			'Authors',
			'Title',
			'Year',
			'Source title',
			'DOI',
			'Abstract',
			'PubMed ID'
		]);
		expect(selection).toEqual({
			title: 'Title',
			abstract_text: 'Abstract',
			authors: 'Authors',
			publication_year: 'Year',
			journal: 'Source title',
			doi: 'DOI',
			pmid: 'PubMed ID'
		});
	});

	it('builds the server mapping from the chosen columns only', () => {
		expect(buildCsvMapping({ title: 'Title', doi: '', journal: undefined })).toEqual({
			title: 'Title'
		});
	});

	it('explains why a mapping cannot be imported yet', () => {
		expect(csvMappingProblems({})).toEqual(['Choose at least one column.']);
		expect(csvMappingProblems({ authors: 'Authors' })).toEqual([
			'Map a Title or DOI column, so each record can be identified.'
		]);
		expect(csvMappingProblems({ title: 'Title' })).toEqual([]);
		expect(csvMappingProblems({ doi: 'DOI' })).toEqual([]);
	});
});

describe('formatImportError', () => {
	it('names the line of a tagged-format error', () => {
		expect(formatImportError('invalid ris import: invalid tagged field at line 7')).toBe(
			'Line 7: Invalid tagged field.'
		);
	});

	it('names the line of a BibTeX error', () => {
		expect(formatImportError('invalid bibtex import: unexpected end of file at line 3')).toBe(
			'Line 3: Unexpected end of file.'
		);
	});

	it('drops the format prefix when no line is known, and keeps other messages', () => {
		expect(formatImportError('invalid csv import: record is malformed')).toBe(
			'Record is malformed'
		);
		expect(formatImportError('CSV imports require csv_mapping')).toBe(
			'CSV imports require csv_mapping'
		);
	});
});

describe('run and article status display', () => {
	it('shows a run that missed some articles as completed with problems, not failed', () => {
		expect(runStatusDisplay('completed', 1)).toEqual({
			label: 'completed with problems',
			variant: 'warning'
		});
	});

	it('keeps a clean completed run and a failed run distinct', () => {
		expect(runStatusDisplay('completed', 0)).toEqual({
			label: 'completed',
			variant: 'default'
		});
		expect(runStatusDisplay('failed', 3)).toEqual({ label: 'failed', variant: 'destructive' });
		expect(runStatusDisplay('running', 0).variant).toBe('secondary');
	});

	it('labels missing articles in plain language', () => {
		expect(ingestionItemStatusLabel('not_found')).toBe('Not found in Crossref');
		expect(ingestionItemStatusVariant('not_found')).toBe('warning');
		expect(ingestionItemStatusVariant('failed')).toBe('destructive');
	});

	it('names the format of a file run', () => {
		expect(importRunFormatLabel('ris')).toBe('RIS file');
		expect(importRunFormatLabel('csv')).toBe('CSV file');
		expect(importRunFormatLabel(null)).toBe('Import');
	});
});

describe('PubMed ID runs', () => {
	it('names a PubMed ID run and each of its IDs in plain language', () => {
		expect(importRunFormatLabel('pmid')).toBe('PubMed IDs');
		expect(pmidItemStatusLabel('imported')).toBe('Imported');
		expect(pmidItemStatusLabel('already_in_project')).toBe('Already in project');
		expect(pmidItemStatusLabel('not_found')).toBe('Not found in PubMed');
		expect(pmidItemStatusLabel('failed')).toBe('Could not be fetched');
		expect(pmidItemStatusLabel('queued')).toBe('Waiting for PubMed');
	});

	it('marks only a missing or failed PubMed ID as a warning or an error', () => {
		expect(pmidItemStatusVariant('imported')).toBe('default');
		expect(pmidItemStatusVariant('already_in_project')).toBe('secondary');
		expect(pmidItemStatusVariant('not_found')).toBe('warning');
		expect(pmidItemStatusVariant('failed')).toBe('destructive');
	});

	it('shows a PubMed ID run that found some articles as completed with problems', () => {
		expect(runStatusDisplay('completed', 2)).toEqual({
			label: 'completed with problems',
			variant: 'warning'
		});
	});
});
