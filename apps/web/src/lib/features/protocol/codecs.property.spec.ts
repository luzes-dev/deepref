import { fc, test } from '@fast-check/vitest';
import { expect } from 'vitest';
import { FRAMEWORK_FIELDS, frameworkFieldsForKind } from './codecs';

const values = fc.dictionary(
	fc.string({ minLength: 1, maxLength: 32 }),
	fc.string({ maxLength: 128 }),
	{ maxKeys: 24 }
);

test.prop({ fields: values })(
	'projects only fields allowed by a structured framework',
	({ fields }) => {
		const projected = frameworkFieldsForKind('pico', fields);

		expect(Object.keys(projected)).toEqual([...FRAMEWORK_FIELDS.pico]);
		for (const field of FRAMEWORK_FIELDS.pico) {
			expect(projected[field]).toBe(fields[field] ?? '');
		}
	}
);

test.prop({ fields: values })('preserves arbitrary fields for custom frameworks', ({ fields }) => {
	const projected = frameworkFieldsForKind('custom', fields);

	expect(projected).toEqual(fields);
	expect(projected).not.toBe(fields);
});
