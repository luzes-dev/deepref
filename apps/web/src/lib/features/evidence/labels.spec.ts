import { describe, expect, it } from 'vitest';
import { citationLabel, humanizeCode, quoteLabel, titleLabel } from './labels';

describe('evidence labels', () => {
	it('quotes a passage in one line and shortens it at a word boundary', () => {
		expect(quoteLabel('  Stroke was\n defined based on clinical features  ')).toBe(
			'“Stroke was defined based on clinical features”'
		);
		const long = 'word '.repeat(40).trim();
		const label = quoteLabel(long) ?? '';
		expect(label.startsWith('“word word')).toBe(true);
		expect(label.endsWith('…”')).toBe(true);
		expect(label.length).toBeLessThanOrEqual(96);
		expect(quoteLabel('   ')).toBeNull();
	});

	it('prefers the quote, then the report title, then the page alone', () => {
		expect(citationLabel({ page: 2, quote: 'Sample size was 51', title: 'Ng 2007' })).toBe(
			'p. 2 · “Sample size was 51”'
		);
		expect(citationLabel({ page: 2, quote: null, title: 'Randomized trial of a sensor' })).toBe(
			'p. 2 · Randomized trial of a sensor'
		);
		expect(citationLabel({ page: 4 })).toBe('p. 4');
		expect(citationLabel({ fallback: 'Evidence on a deleted passage' })).toBe(
			'Evidence on a deleted passage'
		);
	});

	it('never puts an id or a hash into the human label', () => {
		const label = citationLabel({
			page: 3,
			quote: 'Outcome measured at 6 months',
			title: null
		});
		expect(label).not.toMatch(/[0-9a-f]{8}-[0-9a-f]{4}/);
		expect(label).not.toMatch(/hash/i);
	});

	it('shortens titles and humanizes stored codes', () => {
		expect(titleLabel(null)).toBeNull();
		expect(titleLabel('x'.repeat(200))?.length).toBeLessThanOrEqual(71);
		expect(humanizeCode('does_not_meet')).toBe('Does not meet');
		expect(humanizeCode('high_concern')).toBe('High concern');
		expect(humanizeCode('unclear')).toBe('Unclear');
		expect(humanizeCode('')).toBe('');
	});
});
