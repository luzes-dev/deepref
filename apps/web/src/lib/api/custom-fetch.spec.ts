import { afterEach, describe, expect, it, vi } from 'vitest';
import { ApiError, customFetch } from './custom-fetch';

afterEach(() => {
	vi.unstubAllGlobals();
});

describe('customFetch', () => {
	it('rebases generated URLs and returns the response envelope', async () => {
		const fetch = vi.fn().mockResolvedValue(
			new Response(JSON.stringify({ status: 'ok' }), {
				status: 200,
				headers: { 'content-type': 'application/json', 'x-test': 'yes' }
			})
		);
		vi.stubGlobal('fetch', fetch);

		await expect(
			customFetch<{ data: { status: string }; status: 200; headers: Headers }>(
				'http://localhost:8080/health?source=generated',
				{ method: 'GET' }
			)
		).resolves.toMatchObject({
			data: { status: 'ok' },
			status: 200
		});

		const request = fetch.mock.calls[0]?.[0] as Request;
		expect(request.url).toBe('http://localhost:8080/health?source=generated');
	});

	it('returns undefined data for no-content responses', async () => {
		vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 204 })));

		await expect(
			customFetch<{ data: undefined; status: 204; headers: Headers }>(
				'http://localhost:8080/projects/id',
				{ method: 'DELETE' }
			)
		).resolves.toMatchObject({ data: undefined, status: 204 });
	});

	it('throws structured API errors', async () => {
		vi.stubGlobal(
			'fetch',
			vi.fn().mockResolvedValue(
				new Response(JSON.stringify({ error: 'invalid request' }), {
					status: 400,
					statusText: 'Bad Request',
					headers: { 'content-type': 'application/json' }
				})
			)
		);

		await expect(
			customFetch('http://localhost:8080/projects', { method: 'POST' })
		).rejects.toEqual(new ApiError(400, 'invalid request', { error: 'invalid request' }));
	});

	it('preserves request signals', async () => {
		const fetch = vi.fn().mockResolvedValue(
			new Response(JSON.stringify({ status: 'ok' }), {
				status: 200,
				headers: { 'content-type': 'application/json' }
			})
		);
		vi.stubGlobal('fetch', fetch);
		const controller = new AbortController();

		await customFetch('http://localhost:8080/health', {
			method: 'GET',
			signal: controller.signal
		});

		const request = fetch.mock.calls[0]?.[0] as Request;
		controller.abort();
		expect(request.signal.aborted).toBe(true);
	});
});
