import { QueryClient } from '@tanstack/svelte-query';
import { ApiError } from './custom-fetch';

export function shouldRetryQuery(failureCount: number, error: Error): boolean {
	if (failureCount >= 1) return false;
	return !(error instanceof ApiError) || error.status >= 500;
}

export function createAppQueryClient() {
	return new QueryClient({
		defaultOptions: {
			queries: {
				staleTime: 30_000,
				retry: shouldRetryQuery,
				refetchOnWindowFocus: true,
				refetchOnReconnect: true
			},
			mutations: {
				retry: false
			}
		}
	});
}
