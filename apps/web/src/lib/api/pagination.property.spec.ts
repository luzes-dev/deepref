import { fc, test } from '@fast-check/vitest';
import { expect } from 'vitest';
import { cursorUrl } from './pagination';

test.prop({
	cursor: fc.string({ minLength: 1, maxLength: 128 }),
	limit: fc.integer({ min: 1, max: 500 })
})('keeps opaque cursors intact when building page URLs', ({ cursor, limit }) => {
	const url = new URL(cursorUrl('/api/projects', cursor, limit), 'http://localhost');

	expect(url.pathname).toBe('/api/projects');
	expect(url.searchParams.get('limit')).toBe(String(limit));
	expect(url.searchParams.get('cursor')).toBe(cursor);
});
