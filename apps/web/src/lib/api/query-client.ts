import { MutationCache, QueryCache, QueryClient } from '@tanstack/svelte-query';
import { getGetCurrentUserQueryKey } from './generated/auth/auth';
import { ApiError } from './custom-fetch';

export function shouldRetryQuery(failureCount: number, error: Error): boolean {
	if (failureCount >= 1) return false;
	return !(error instanceof ApiError) || error.status >= 500;
}

/** An unauthenticated answer to anything but `/me` means the session ended; re-ask `/me` so the shell shows the sign-in screen. */
function isSessionEnded(error: unknown): boolean {
	return error instanceof ApiError && error.status === 401;
}

export function createAppQueryClient() {
	const recheckSession = (error: unknown) => {
		if (isSessionEnded(error)) {
			void client.invalidateQueries({ queryKey: getGetCurrentUserQueryKey() });
		}
	};
	const client: QueryClient = new QueryClient({
		queryCache: new QueryCache({
			onError: (error, query) => {
				if (query.queryKey[0] !== getGetCurrentUserQueryKey()[0]) recheckSession(error);
			}
		}),
		mutationCache: new MutationCache({ onError: recheckSession }),
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
	return client;
}
