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

// fallow-ignore-next-line unused-type
export type ErrorType<T> = ApiError<T>;
// fallow-ignore-next-line unused-type
export type BodyType<T> = T;

function requestUrl(contextUrl: string): string {
	const generatedUrl = new URL(contextUrl, 'http://localhost');
	const path = `${generatedUrl.pathname}${generatedUrl.search}`;

	return typeof window === 'undefined' ? new URL(path, 'http://localhost').toString() : path;
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
