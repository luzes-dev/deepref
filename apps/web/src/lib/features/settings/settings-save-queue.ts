import type { QueryClient } from '@tanstack/svelte-query';

type QueueTail = Promise<void>;

// A query client is scoped to one browser application (and to one SSR request
// when rendering on the server), so it is a safe owner for the shared queue.
// Keeping the queue in a WeakMap avoids cross-request state while allowing a
// modal and the full settings route to serialize their writes.
const queueTails = new WeakMap<QueryClient, QueueTail>();

export function enqueueSettingsSave<T>(
	queryClient: QueryClient,
	operation: () => Promise<T>
): Promise<T> {
	const previous = queueTails.get(queryClient) ?? Promise.resolve();
	const current = previous.catch(() => undefined).then(operation);
	queueTails.set(
		queryClient,
		current.then(
			() => undefined,
			() => undefined
		)
	);
	return current;
}
