<script lang="ts">
	import BellIcon from '@lucide/svelte/icons/bell';
	import BellRingIcon from '@lucide/svelte/icons/bell-ring';
	import { useQueryClient } from '@tanstack/svelte-query';
	import * as Popover from '@deepref/ui/popover';
	import { Spinner } from '@deepref/ui/spinner';
	import { cn } from '@deepref/ui/utils';
	import {
		createGetUnreadNotificationCount,
		createListNotifications,
		createMarkNotificationsRead,
		getGetUnreadNotificationCountQueryKey
	} from '$lib/api/generated/notifications/notifications';
	import {
		setNotificationPanelOpen,
		relativeTime
	} from '$lib/features/notifications/state.svelte';

	const queryClient = useQueryClient();
	const unreadQuery = createGetUnreadNotificationCount(() => ({
		query: { refetchInterval: 10_000, refetchIntervalInBackground: false, staleTime: 5_000 }
	}));
	const listQuery = createListNotifications(() => ({
		query: { enabled: false, staleTime: 5_000 }
	}));
	const markRead = createMarkNotificationsRead();

	let open = $state(false);

	const unreadCount = $derived(unreadQuery.data?.data.count ?? 0);
	const items = $derived(listQuery.data?.data.items ?? []);

	function handleOpenChange(next: boolean): void {
		open = next;
		setNotificationPanelOpen(next);
		if (!next) return;
		void listQuery.refetch();
		void markRead.mutateAsync(
			{ data: { all: true } },
			{
				onSettled: () => {
					void queryClient.invalidateQueries({
						queryKey: getGetUnreadNotificationCountQueryKey()
					});
					void listQuery.refetch();
				}
			}
		);
	}

	function severityClass(severity: string): string {
		switch (severity) {
			case 'success':
				return 'bg-emerald-500';
			case 'warning':
				return 'bg-amber-500';
			case 'error':
				return 'bg-destructive';
			default:
				return 'bg-sky-500';
		}
	}
</script>

<Popover.Root {open} onOpenChange={handleOpenChange}>
	<Popover.Trigger>
		{#snippet child({ props })}
			<button
				{...props}
				type="button"
				class="relative inline-flex size-8 items-center justify-center rounded-md text-muted-foreground transition-colors hover:bg-muted/70 hover:text-foreground focus-visible:ring-1 focus-visible:ring-ring focus-visible:outline-hidden"
				aria-label={unreadCount > 0
					? `Notifications, ${unreadCount} unread`
					: 'Notifications'}
				data-testid="notifications-button"
			>
				{#if unreadCount > 0}
					<BellRingIcon class="size-4" aria-hidden="true" />
					<span
						class="absolute top-1 right-1 size-2 rounded-full bg-destructive"
						data-testid="notifications-unread-dot"
					></span>
					<span class="sr-only">{unreadCount} unread</span>
				{:else}
					<BellIcon class="size-4" aria-hidden="true" />
				{/if}
			</button>
		{/snippet}
	</Popover.Trigger>
	<Popover.Content align="end" class="w-88 p-0" data-testid="notifications-panel">
		<header class="flex items-center justify-between border-b border-border/70 px-4 py-3">
			<span class="text-sm font-semibold">Notifications</span>
			<span class="text-xs text-muted-foreground" data-testid="notifications-count">
				{unreadCount > 0 ? `${unreadCount} unread` : 'All caught up'}
			</span>
		</header>
		<div class="max-h-80 overflow-y-auto">
			{#if listQuery.isFetching && items.length === 0}
				<div
					class="flex items-center justify-center gap-2 px-4 py-8 text-sm text-muted-foreground"
				>
					<Spinner class="size-4" /> Loading notifications…
				</div>
			{:else if items.length === 0}
				<p
					class="px-4 py-8 text-center text-sm text-muted-foreground"
					data-testid="notifications-empty"
				>
					Nothing here yet. Finished imports, review runs, and automations will show up
					here.
				</p>
			{:else}
				<ul class="divide-y divide-border/60" data-testid="notifications-list">
					{#each items as notification (notification.id)}
						{@const isUnread = notification.read_at === null}
						<li
							class={cn(
								'flex gap-3 px-4 py-3',
								isUnread ? 'bg-muted/40' : 'bg-transparent'
							)}
							data-testid="notifications-item"
							data-severity={notification.severity}
						>
							<span
								class={cn(
									'mt-1.5 size-2 shrink-0 rounded-full',
									severityClass(notification.severity)
								)}
								aria-hidden="true"
							></span>
							<div class="min-w-0 flex-1">
								<p class="text-sm leading-5 font-medium">{notification.title}</p>
								{#if notification.body?.trim()}
									<p class="mt-0.5 text-sm leading-5 text-muted-foreground">
										{notification.body}
									</p>
								{/if}
								<p class="mt-1 text-xs text-muted-foreground">
									{relativeTime(notification.created_at)}
								</p>
							</div>
							{#if isUnread}
								<span class="sr-only">Unread</span>
							{/if}
						</li>
					{/each}
				</ul>
			{/if}
		</div>
	</Popover.Content>
</Popover.Root>
