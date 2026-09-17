<script lang="ts">
	import { createListNotifications } from '$lib/api/generated/notifications/notifications';
	import { isNotificationPanelOpen, observeNotifications } from './state.svelte';
	import { notifyNotification } from './toast';

	const listQuery = createListNotifications(() => ({
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
