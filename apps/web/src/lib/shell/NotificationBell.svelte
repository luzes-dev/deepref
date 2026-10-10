<script lang="ts">
	import BellIcon from '@lucide/svelte/icons/bell';
	import BellRingIcon from '@lucide/svelte/icons/bell-ring';
	import {
		createInfiniteQuery,
		infiniteQueryOptions,
		useQueryClient
	} from '@tanstack/svelte-query';
	import * as Popover from '@deepref/ui/popover';
	import { Button } from '@deepref/ui/button';
	import { Spinner } from '@deepref/ui/spinner';
	import { cn } from '@deepref/ui/utils';
	import type { NotificationDto } from '#lib/api/generated/models/index.js';
	import {
		createGetUnreadNotificationCount,
		createMarkNotificationsRead,
		getGetUnreadNotificationCountQueryKey,
		listNotifications
	} from '#lib/api/generated/notifications/notifications.js';
	import {
		notificationHref,
		plainNotificationText
	} from '#lib/features/notifications/notification-copy.js';
	import {
		relativeTime,
		setNotificationPanelOpen
	} from '#lib/features/notifications/state.svelte.js';

	let { projectId = null }: { projectId?: string | null } = $props();

	const PAGE_SIZE = 20;

	const queryClient = useQueryClient();
	let open = $state(false);
	let scope = $state<'project' | 'all'>('project');

	// Without a project in view the bell can only show every project.
	const projectScoped = $derived(Boolean(projectId) && scope === 'project');
	const scopeProjectId = $derived(projectScoped ? (projectId ?? undefined) : undefined);

	// The total is the real unread count for the scope, never a page length.
	const unreadQuery = createGetUnreadNotificationCount(
		() => ({ project_id: scopeProjectId }),
		() => ({
			query: { refetchInterval: 10_000, refetchIntervalInBackground: false, staleTime: 5_000 }
		})
	);
	const markRead = createMarkNotificationsRead();

	// One cached feed per scope; pages follow the server's cursor.
	function feedOptions(scopedProjectId: string | undefined) {
		return infiniteQueryOptions({
			queryKey: ['notifications', 'feed', scopedProjectId ?? 'all'] as const,
			queryFn: async ({ pageParam }) => {
				const response = await listNotifications({
					project_id: scopedProjectId,
					cursor: pageParam,
					limit: PAGE_SIZE
				});
				return response.data;
			},
			initialPageParam: undefined as string | undefined,
			getNextPageParam: (last) => last.next_cursor ?? undefined
		});
	}

	const feed = createInfiniteQuery(() => ({
		...feedOptions(scopeProjectId),
		enabled: open,
		staleTime: 5_000
	}));

	const unreadCount = $derived(unreadQuery.data?.data.count ?? 0);
	const items = $derived(feed.data?.pages.flatMap((page) => page.items) ?? []);

	function setOpen(next: boolean): void {
		open = next;
		setNotificationPanelOpen(next);
	}

	async function markRows(request: {
		ids?: string[];
		all?: boolean;
		project_id?: string;
	}): Promise<void> {
		try {
			await markRead.mutateAsync({ data: request });
		} finally {
			void queryClient.invalidateQueries({
				queryKey: getGetUnreadNotificationCountQueryKey()
			});
			// Every scope's feed is stale once rows change state.
			void queryClient.invalidateQueries({
				predicate: (query) => query.queryKey[1] === 'feed'
			});
		}
	}

	function markAllRead(): void {
		void markRows({ all: true, project_id: scopeProjectId }).catch(() => undefined);
	}

	// Opening a row reads it and follows its link; the rest stay unread until
	// they are opened or "Mark all read" is pressed.
	function openRow(notification: NotificationDto, linked: boolean): void {
		if (!notification.read_at) {
			void markRows({ ids: [notification.id] }).catch(() => undefined);
		}
		// Close after the link's own click has been handed to the router, so the
		// anchor is not removed mid-navigation.
		if (linked) setTimeout(() => setOpen(false), 0);
	}

	function severityClass(severity: string): string {
		switch (severity) {
			case 'success':
				return 'bg-success';
			case 'warning':
				return 'bg-warning';
			case 'error':
				return 'bg-destructive';
			default:
				return 'bg-info';
		}
	}
</script>

<Popover.Root {open} onOpenChange={setOpen}>
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
	<Popover.Content
		align="end"
		class="w-[min(24rem,calc(100vw-2rem))] gap-0 p-0"
		data-testid="notifications-panel"
	>
		<header class="flex flex-col gap-2 border-b border-border/70 px-4 py-3">
			<div class="flex items-center justify-between gap-3">
				<span class="text-sm font-semibold">Notifications</span>
				<span
					class="text-xs text-muted-foreground tabular-nums"
					data-testid="notifications-count"
				>
					{unreadCount > 0 ? `${unreadCount} unread` : 'All caught up'}
				</span>
			</div>
			<div class="flex items-center justify-between gap-2">
				{#if projectId}
					<div
						class="inline-flex gap-0.5 rounded-md border border-border/70 p-0.5"
						role="group"
						aria-label="Which projects to show"
					>
						<Button
							variant={projectScoped ? 'secondary' : 'ghost'}
							size="xs"
							aria-pressed={projectScoped}
							onclick={() => (scope = 'project')}
							data-testid="notifications-scope-project"
						>
							This project
						</Button>
						<Button
							variant={projectScoped ? 'ghost' : 'secondary'}
							size="xs"
							aria-pressed={!projectScoped}
							onclick={() => (scope = 'all')}
							data-testid="notifications-scope-all"
						>
							All projects
						</Button>
					</div>
				{:else}
					<span class="text-xs text-muted-foreground">All projects</span>
				{/if}
				<Button
					variant="ghost"
					size="xs"
					disabled={unreadCount === 0 || markRead.isPending}
					onclick={markAllRead}
					data-testid="notifications-mark-all"
				>
					Mark all read
				</Button>
			</div>
		</header>

		<div class="max-h-96 overflow-y-auto">
			{#if feed.isPending}
				<div
					class="flex items-center justify-center gap-2 px-4 py-8 text-sm text-muted-foreground"
				>
					<Spinner class="size-4" /> Loading notifications…
				</div>
			{:else if feed.isError}
				<div class="grid justify-items-start gap-2 px-4 py-6 text-sm">
					<p class="text-muted-foreground">Notifications could not be loaded.</p>
					<Button variant="outline" size="sm" onclick={() => void feed.refetch()}>
						Try again
					</Button>
				</div>
			{:else if items.length === 0}
				<p
					class="px-4 py-8 text-center text-sm text-muted-foreground"
					data-testid="notifications-empty"
				>
					{projectScoped
						? 'Nothing for this project yet. Finished imports, review runs, and automations will show up here.'
						: 'Nothing here yet. Finished imports, review runs, and automations will show up here.'}
				</p>
			{:else}
				<ul class="divide-y divide-border/60" data-testid="notifications-list">
					{#each items as notification (notification.id)}
						{@const copy = plainNotificationText(notification)}
						{@const href = notificationHref(notification)}
						{@const unread = !notification.read_at}
						<li
							class={cn('flex flex-col', unread ? 'bg-muted/40' : 'bg-transparent')}
							data-testid="notifications-item"
							data-severity={notification.severity}
							data-read={unread ? 'false' : 'true'}
						>
							{#snippet rowBody()}
								<span
									class={cn(
										'mt-1.5 size-2 shrink-0 rounded-full',
										severityClass(notification.severity)
									)}
									aria-hidden="true"
								></span>
								<div class="min-w-0 flex-1">
									<p
										class={cn(
											'text-sm leading-5',
											unread ? 'font-semibold' : 'font-medium'
										)}
									>
										{copy.title}
									</p>
									{#if copy.body}
										<p class="mt-0.5 text-sm leading-5 text-muted-foreground">
											{copy.body}
										</p>
									{/if}
									<p
										class="mt-1 flex flex-wrap items-center gap-x-2 gap-y-1 text-xs text-muted-foreground"
									>
										{#if notification.project_name}
											<span
												class="rounded-sm bg-muted px-1.5 py-0.5 text-2xs font-medium text-foreground"
												data-testid="notifications-project"
											>
												{notification.project_name}
											</span>
										{/if}
										<span>{relativeTime(notification.created_at)}</span>
										{#if unread}<span class="sr-only">Unread.</span>{/if}
									</p>
								</div>
							{/snippet}
							{#if href}
								<a
									{href}
									class="flex gap-3 px-4 pt-3 pb-2 transition-colors hover:bg-muted/60 focus-visible:ring-1 focus-visible:ring-ring focus-visible:outline-hidden"
									data-testid="notifications-item-link"
									onclick={() => openRow(notification, true)}
								>
									{@render rowBody()}
								</a>
							{:else}
								<button
									type="button"
									class="flex w-full gap-3 px-4 pt-3 pb-2 text-left transition-colors hover:bg-muted/60 focus-visible:ring-1 focus-visible:ring-ring focus-visible:outline-hidden"
									onclick={() => openRow(notification, false)}
								>
									{@render rowBody()}
								</button>
							{/if}
							{#if copy.technical}
								<details class="px-4 pb-3 text-xs text-muted-foreground">
									<summary class="cursor-pointer select-none"
										>Technical details</summary
									>
									<p
										class="mt-1 font-mono text-2xs break-words whitespace-pre-line"
									>
										{copy.technical}
									</p>
								</details>
							{/if}
						</li>
					{/each}
				</ul>
				{#if feed.hasNextPage}
					<div class="border-t border-border/60 p-2">
						<Button
							variant="ghost"
							size="sm"
							class="w-full"
							disabled={feed.isFetchingNextPage}
							onclick={() => void feed.fetchNextPage()}
							data-testid="notifications-load-more"
						>
							{feed.isFetchingNextPage ? 'Loading…' : 'Load more'}
						</Button>
					</div>
				{/if}
			{/if}
		</div>
	</Popover.Content>
</Popover.Root>
