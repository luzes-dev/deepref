<script lang="ts">
	import { createGetDependencyStatus } from '$lib/api/generated/health/health';
	import { notifyError, notifySuccess, notifyWarning } from '$lib/features/notifications/toast';
	import { observeDependencyHealth, type DependencyHealthObservation } from './dependency-health';

	const dependenciesQuery = createGetDependencyStatus(() => ({
		query: {
			refetchInterval: 10_000,
			refetchIntervalInBackground: false,
			refetchOnWindowFocus: 'always',
			staleTime: 5_000
		}
	}));

	let previous: DependencyHealthObservation = { error: false, states: null };

	const refreshAction = () => ({
		label: 'Refresh',
		onClick: () => void dependenciesQuery.refetch()
	});

	$effect(() => {
		const status = dependenciesQuery.data?.data;
		const error = dependenciesQuery.error;
		previous = observeDependencyHealth(previous, status, error, {
			onDegraded: (name, summary, coreUnavailable) => {
				const line = `${name}: ${summary}`;
				const action = refreshAction();
				if (coreUnavailable) {
					notifyError('Core service interruption', undefined, line, { action });
				} else {
					notifyWarning(
						'Some features are degraded',
						`${line}. Projects, articles, and ingestions remain available while durable jobs drain.`,
						{ action }
					);
				}
			},
			onRecovered: () => notifySuccess('All systems operational'),
			onStatusError: (message) =>
				notifyError('Dependency status unavailable', undefined, message, {
					action: refreshAction()
				})
		});
	});
</script>
