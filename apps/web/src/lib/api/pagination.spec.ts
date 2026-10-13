import { afterEach, describe, expect, it, vi } from 'vitest';
import { cursorUrl, fetchAllDocumentBlocks } from './pagination';

afterEach(() => vi.unstubAllGlobals());

describe('cursorUrl', () => {
	it('adds an opaque cursor without decoding or rewriting it', () => {
		expect(cursorUrl('/api/projects', 'opaque+/=cursor', 25)).toBe(
			'/api/projects?limit=25&cursor=opaque%2B%2F%3Dcursor'
		);
	});

	it('omits the cursor for the initial page', () => {
		expect(cursorUrl('/api/projects/project-1/acquisitions', undefined, 50)).toBe('/api/projects/project-1/acquisitions?limit=50');
	});
});

describe('document block pagination', () => {
	it('collects later pages before returning overlays and preserves the abort signal', async () => {
		const requests: Request[] = [];
		vi.stubGlobal('fetch', async (request: Request) => {
			requests.push(request);
			const page = requests.length === 1
				? { items: Array.from({ length: 100 }, (_, id) => ({ id: String(id) })), next_cursor: 'opaque+/=cursor' }
				: { items: [{ id: '100' }, { id: '101' }], next_cursor: null };
			return new Response(JSON.stringify(page), { headers: { 'content-type': 'application/json' } });
		});
		const controller = new AbortController();
		const response = await fetchAllDocumentBlocks('project', 'report', 'document', 100, controller.signal);
		expect(response.data).toHaveLength(102);
		expect(response.data.at(-1)?.id).toBe('101');
		expect(requests).toHaveLength(2);
		expect(new URL(requests[1]!.url).searchParams.get('cursor')).toBe('opaque+/=cursor');
		controller.abort();
		expect(requests.every((request) => request.signal.aborted)).toBe(true);
	});

	it('does not return a misleading partial document when a later page fails', async () => {
		let calls = 0;
		vi.stubGlobal('fetch', async () => {
			calls += 1;
			return calls === 1
				? new Response(JSON.stringify({ items: [{ id: 'first' }], next_cursor: 'next' }), { headers: { 'content-type': 'application/json' } })
				: new Response(JSON.stringify({ message: 'unavailable' }), { status: 503, headers: { 'content-type': 'application/json' } });
		});
		await expect(fetchAllDocumentBlocks('project', 'report', 'document')).rejects.toMatchObject({ status: 503 });
	});
});
