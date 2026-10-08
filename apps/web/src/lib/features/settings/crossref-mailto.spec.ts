import { describe, expect, it } from 'vitest';
import {
	CROSSREF_MAILTO_INVALID,
	CROSSREF_MAILTO_REQUIRED,
	crossrefMailtoError
} from './crossref-mailto';

describe('crossrefMailtoError', () => {
	it('accepts contact addresses, ignoring surrounding spaces', () => {
		for (const value of [
			'research@example.org',
			'  research@example.org  ',
			'first.last+tag@univ.ac.uk',
			'a@xn--e1afmkfd.xn--p1ai'
		]) {
			expect(crossrefMailtoError(value), value).toBeUndefined();
		}
	});

	it('asks for a value when the field is blank', () => {
		expect(crossrefMailtoError('')).toBe(CROSSREF_MAILTO_REQUIRED);
		expect(crossrefMailtoError('   ')).toBe(CROSSREF_MAILTO_REQUIRED);
	});

	it('rejects malformed addresses with the format hint', () => {
		for (const value of [
			'not-an-email',
			'a@b',
			'a@b.',
			'@example.org',
			'a b@example.org',
			'a@@example.org',
			'a@.example.org',
			'a@example..org',
			'a@example.o',
			`${'a'.repeat(250)}@example.org`
		]) {
			expect(crossrefMailtoError(value), value).toBe(CROSSREF_MAILTO_INVALID);
		}
	});
});
