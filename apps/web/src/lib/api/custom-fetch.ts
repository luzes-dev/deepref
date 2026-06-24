const publicEnv = import.meta.env as Record<string, string | undefined>;

declare global {
	interface Window {
		__DEEPREF_CONFIG__?: {
			apiBaseUrl?: string;
		};
	}
}

const runtimeApiBase =
	typeof window === 'undefined' ? undefined : window.__DEEPREF_CONFIG__?.apiBaseUrl;

export const apiBase = runtimeApiBase || publicEnv.PUBLIC_API_BASE_URL || 'http://localhost:8080';

export class ApiError<T = unknown> extends Error {
	readonly status: number;
	readonly info: T;

	constructor(status: number, message: string, info: T) {
		super(message);
		this.name = 'ApiError';
		this.status = status;
		this.info = info;
	}
}

export type ErrorType<T> = ApiError<T>;
export type BodyType<T> = T;

function requestUrl(contextUrl: string): string {
	const generatedUrl = new URL(contextUrl);
	return new URL(`${generatedUrl.pathname}${generatedUrl.search}`, apiBase).toString();
}

async function responseBody(response: Response): Promise<unknown> {
	if (response.status === 204) return undefined;

	const contentType = response.headers.get('content-type') ?? '';
	if (contentType.includes('application/json')) return response.json();
	if (contentType.includes('application/pdf')) return response.blob();
	return response.text();
}

function errorMessage(body: unknown, fallback: string): string {
	if (
		typeof body === 'object' &&
		body !== null &&
		'error' in body &&
		typeof body.error === 'string'
	) {
		return body.error;
	}
	if (typeof body === 'string' && body) return body;
	return fallback;
}

export async function customFetch<T>(url: string, options: RequestInit): Promise<T> {
	const headers = new Headers(options.headers);
	const response = await fetch(
		new Request(requestUrl(url), {
			...options,
			headers
		})
	);
	const data = await responseBody(response);

	if (!response.ok) {
		throw new ApiError(response.status, errorMessage(data, response.statusText), data);
	}

	return {
		data,
		status: response.status,
		headers: response.headers
	} as T;
}
