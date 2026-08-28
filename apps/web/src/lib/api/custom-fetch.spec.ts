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
				'/api/health?source=generated',
				{ method: 'GET' }
			)
		).resolves.toMatchObject({
			data: { status: 'ok' },
			status: 200
		});

		const request = fetch.mock.calls[0]?.[0] as Request;
		expect(request.url).toBe('http://localhost/api/health?source=generated');
	});

	it('returns undefined data for no-content responses', async () => {
		vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(null, { status: 204 })));

		await expect(
			customFetch<{ data: undefined; status: 204; headers: Headers }>('/api/projects/id', {
				method: 'DELETE'
			})
		).resolves.toMatchObject({ data: undefined, status: 204 });
	});

	it('returns every attachment as a Blob and preserves download metadata', async () => {
		const bytes = new Uint8Array([0, 1, 2, 255]);
		vi.stubGlobal(
			'fetch',
			vi.fn().mockResolvedValue(
				new Response(bytes, {
					status: 200,
					headers: {
						'content-type': 'text/csv; charset=utf-8',
						'content-disposition': 'attachment; filename="reports.csv"'
					}
				})
			)
		);

		const response = await customFetch<{ data: Blob; status: 200; headers: Headers }>(
			'/api/projects/project/exports/reports.csv',
			{ method: 'GET' }
		);
		expect(response.data).toBeInstanceOf(Blob);
		expect(response.data.type).toMatch(/^text\/csv(?:;|$)/);
		expect(new Uint8Array(await response.data.arrayBuffer())).toEqual(bytes);
		expect(response.headers.get('content-disposition')).toContain('reports.csv');
	});

	it('throws structured API errors', async () => {
		vi.stubGlobal(
			'fetch',
			vi.fn().mockResolvedValue(
				new Response(
					JSON.stringify({
						code: 'INTERNAL_ERROR',
						message: 'request failed',
						correlation_id: 'body-correlation'
					}),
					{
						status: 500,
						statusText: 'Service Unavailable',
						headers: {
							'content-type': 'application/json',
							'x-correlation-id': 'response-correlation',
							'x-request-id': 'request-123'
						}
					}
				)
			)
		);

		const error = await customFetch('/api/projects/test/graph', { method: 'GET' }).catch(
			(reason: unknown) => reason
		);

		expect(error).toBeInstanceOf(ApiError);
		expect(error).toMatchObject({
			status: 500,
			code: 'INTERNAL_ERROR',
			message: 'request failed',
			retryAfter: undefined,
			retryAfterSeconds: undefined,
			correlationId: 'body-correlation',
			requestId: 'request-123'
		});
		expect((error as ApiError).requestHeaders.get('x-correlation-id')).toBeTruthy();
		expect((error as ApiError).responseHeaders.get('retry-after')).toBeNull();
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

		await customFetch('/api/health', {
			method: 'GET',
			signal: controller.signal
		});

		const request = fetch.mock.calls[0]?.[0] as Request;
		controller.abort();
		expect(request.signal.aborted).toBe(true);
	});
});
