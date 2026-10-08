import { describe, expect, it } from 'vitest';
import { isStudyExtracted } from './synthesis-progress';

describe('study extraction completeness', () => {
	const required = [{ id: 'f1' }, { id: 'f2' }];

	it('needs a value for every required field', () => {
		expect(isStudyExtracted(required, [{ field_definition_id: 'f1' }])).toBe(false);
		expect(
			isStudyExtracted(required, [
				{ field_definition_id: 'f1' },
				{ field_definition_id: 'f2' }
			])
		).toBe(true);
	});

	it('treats a study as extracted when no field is required', () => {
		expect(isStudyExtracted([], [])).toBe(true);
	});
});
