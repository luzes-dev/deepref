import { http, HttpResponse, delay } from 'msw';
import type { SettingsDto, ProjectDto } from '$lib/api/generated/models';

export const mockSettings: SettingsDto = {
	citation_provider: 'crossref',
	metadata_provider: 'crossref',
	crossref_mailto: 'researcher@example.com',
	default_max_depth: 2,
	max_concurrency: 4,
	rate_limit_per_second: 10,
	retry_attempts: 3
};

export const mockProjects: ProjectDto[] = [
	{
		id: '11111111-1111-1111-1111-111111111111',
		name: 'Systematic Review of ML',
		description: 'Literature review of ML methods in medicine',
		default_max_depth: 2,
		created_at: '2026-01-01T00:00:00Z',
		updated_at: '2026-01-02T00:00:00Z'
	}
];

export const handlers = [
	http.get('/api/settings', () => {
		return HttpResponse.json({ data: mockSettings, status: 200 });
	}),
	http.put('/api/settings', async ({ request }) => {
		const body = (await request.json()) as Partial<SettingsDto>;
		return HttpResponse.json({
			data: { ...mockSettings, ...body },
			status: 200
		});
	}),
	http.get('/api/projects', () => {
		return HttpResponse.json({ data: mockProjects, status: 200 });
	}),
	http.get('/api/projects/:projectId', ({ params }) => {
		const project = mockProjects.find((p) => p.id === params.projectId) ?? mockProjects[0];
		return HttpResponse.json({ data: project, status: 200 });
	})
];

export const networkOverrides = {
	loading: (
		path: string,
		method: 'get' | 'post' | 'put' | 'delete' = 'get',
		delayMs: number = 10_000
	) => {
		return http[method](path, async () => {
			await delay(delayMs);
			return HttpResponse.json({ data: null });
		});
	},
	success: <T>(path: string, data: T, method: 'get' | 'post' | 'put' | 'delete' = 'get') => {
		return http[method](path, () => {
			return HttpResponse.json({ data, status: 200 });
		});
	},
	empty: (path: string, method: 'get' | 'post' | 'put' | 'delete' = 'get') => {
		return http[method](path, () => {
			return HttpResponse.json({ data: [], total: 0, status: 200 });
		});
	},
	validationError: (
		path: string,
		errors: Record<string, string[]>,
		method: 'get' | 'post' | 'put' | 'delete' = 'post'
	) => {
		return http[method](path, () => {
			return HttpResponse.json(
				{
					type: 'https://tools.ietf.org/html/rfc9457',
					title: 'Validation Error',
					status: 400,
					detail: 'One or more fields failed validation',
					errors
				},
				{ status: 400 }
			);
		});
	},
	httpFailure: (
		path: string,
		status: number = 500,
		message: string = 'Internal Server Error',
		method: 'get' | 'post' | 'put' | 'delete' = 'get'
	) => {
		return http[method](path, () => {
			return HttpResponse.json(
				{
					type: 'https://tools.ietf.org/html/rfc9457',
					title: 'Internal Error',
					status,
					detail: message
				},
				{ status }
			);
		});
	},
	retryableFailure: (path: string, method: 'get' | 'post' | 'put' | 'delete' = 'get') => {
		return http[method](path, () => {
			return HttpResponse.json(
				{
					type: 'https://tools.ietf.org/html/rfc9457',
					title: 'Service Temporarily Unavailable',
					status: 503,
					detail: 'Downstream provider rate limited or service busy'
				},
				{ status: 503 }
			);
		});
	},
	degradedResponse: <T>(
		path: string,
		partialData: T,
		method: 'get' | 'post' | 'put' | 'delete' = 'get'
	) => {
		return http[method](path, () => {
			return HttpResponse.json({
				data: partialData,
				status: 200,
				degraded: true,
				degraded_reasons: ['Graph calculation timed out; returning cached projection']
			});
		});
	}
};
