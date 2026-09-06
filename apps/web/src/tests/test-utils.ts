import { QueryClient } from '@tanstack/svelte-query';

export function createTestQueryClient(): QueryClient {
	return new QueryClient({
		defaultOptions: {
			queries: {
				retry: false,
				gcTime: Infinity
			},
			mutations: {
				retry: false
			}
		}
	});
}
