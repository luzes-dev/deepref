import { describe, expect, it } from 'vitest';
import { cursorUrl } from './pagination';

describe('cursorUrl', () => {
	it('adds an opaque cursor without decoding or rewriting it', () => {
		expect(cursorUrl('/api/projects', 'opaque+/=cursor', 25)).toBe(
			'/api/projects?limit=25&cursor=opaque%2B%2F%3Dcursor'
		);
	});

	it('omits the cursor for the initial page', () => {
		expect(cursorUrl('/api/ingestions', undefined, 50)).toBe('/api/ingestions?limit=50');
	});
});
