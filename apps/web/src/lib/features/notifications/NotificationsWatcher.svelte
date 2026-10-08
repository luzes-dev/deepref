<script lang="ts">
	import { createListNotifications } from '$lib/api/generated/notifications/notifications';
	import { isNotificationPanelOpen, observeNotifications } from './state.svelte';
	import { notifyNotification } from './toast';

	// Toasts announce arrivals in every project, so this watcher reads the
	// unscoped feed; the bell's own panel is scoped separately.
	const listQuery = createListNotifications(undefined, () => ({
		query: {
			refetchInterval: 10_000,
			refetchIntervalInBackground: false,
			staleTime: 5_000
		}
	}));

	$effect(() => {
		const items = listQuery.data?.data.items;
		if (!items || isNotificationPanelOpen()) return;
		for (const notification of observeNotifications(items)) {
			notifyNotification(notification);
		}
	});
</script>
