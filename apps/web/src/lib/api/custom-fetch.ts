export class ApiError<T = unknown> extends Error {
	readonly status: number;
	readonly info: T;
	readonly code?: string;
	readonly retryAfter?: string;
	readonly retryAfterSeconds?: number;
	readonly requestId?: string;
	readonly correlationId?: string;
	readonly requestHeaders: Headers;
	readonly responseHeaders: Headers;

	constructor(
		status: number,
		message: string,
		info: T,
		metadata: {
			requestHeaders?: Headers;
			responseHeaders?: Headers;
		} = {}
	) {
		super(message);
		this.name = 'ApiError';
		this.status = status;
		this.info = info;
		this.requestHeaders = metadata.requestHeaders ?? new Headers();
		this.responseHeaders = metadata.responseHeaders ?? new Headers();
		this.code = bodyString(info, 'code');
		this.retryAfter = this.responseHeaders.get('retry-after') ?? undefined;
		this.retryAfterSeconds = retryAfterSeconds(this.retryAfter);
		this.requestId =
			this.responseHeaders.get('x-request-id') ??
			this.requestHeaders.get('x-request-id') ??
			undefined;
		this.correlationId =
			bodyString(info, 'correlation_id') ??
			this.responseHeaders.get('x-correlation-id') ??
			this.requestHeaders.get('x-correlation-id') ??
			undefined;
	}
}

// fallow-ignore-next-line unused-type -- Required by Orval OpenAPI client custom mutator signature
export type ErrorType<T> = ApiError<T>;
// fallow-ignore-next-line unused-type -- Required by Orval OpenAPI client custom mutator signature
export type BodyType<T> = T;

function requestUrl(contextUrl: string): string {
	const generatedUrl = new URL(contextUrl, 'http://localhost');
	const path = `${generatedUrl.pathname}${generatedUrl.search}`;

	return typeof window === 'undefined' ? new URL(path, 'http://localhost').toString() : path;
}

async function responseBody(response: Response): Promise<unknown> {
	if (response.status === 204) return undefined;

	if (/\battachment\b/i.test(response.headers.get('content-disposition') ?? '')) {
		return response.blob();
	}

	const contentType = response.headers.get('content-type') ?? '';
	if (contentType.includes('application/json')) return response.json();
	if (contentType.includes('application/pdf')) return response.blob();
	return response.text();
}

function bodyString(body: unknown, key: string): string | undefined {
	if (typeof body !== 'object' || body === null || !(key in body)) return undefined;
	const value = (body as Record<string, unknown>)[key];
	return typeof value === 'string' && value.length > 0 ? value : undefined;
}

function errorMessage(body: unknown, fallback: string): string {
	const message = bodyString(body, 'message') ?? bodyString(body, 'error');
	if (message) return message;
	if (typeof body === 'string' && body) return body;
	return fallback;
}

function retryAfterSeconds(value: string | undefined): number | undefined {
	if (!value) return undefined;
	const seconds = Number(value);
	if (Number.isFinite(seconds) && seconds >= 0) return seconds;
	const timestamp = Date.parse(value);
	return Number.isNaN(timestamp)
		? undefined
		: Math.max(0, Math.ceil((timestamp - Date.now()) / 1000));
}

function correlationId(): string | undefined {
	return globalThis.crypto?.randomUUID?.();
}

export async function customFetch<T>(url: string, options: RequestInit): Promise<T> {
	const headers = new Headers(options.headers);
	if (!headers.has('x-correlation-id')) {
		const id = correlationId();
		if (id) headers.set('x-correlation-id', id);
	}
	// fallow-ignore-next-line security-sink -- Central fetch wrapper targets verified API client endpoints
	const response = await fetch(
		new Request(requestUrl(url), {
			...options,
			headers
		})
	);
	const data = await responseBody(response);

	if (!response.ok) {
		throw new ApiError(response.status, errorMessage(data, response.statusText), data, {
			requestHeaders: headers,
			responseHeaders: response.headers
		});
	}

	return {
		data,
		status: response.status,
		headers: response.headers
	} as T;
}
