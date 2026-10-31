<script lang="ts">
	import { createInfiniteQuery } from '@tanstack/svelte-query';
	import { Button } from '@deepref/ui/button';
	import * as Select from '@deepref/ui/select';
	import * as Field from '@deepref/ui/field';
	import {
		listSettingsWorkspaces,
		getListSettingsWorkspacesQueryKey
	} from '#lib/api/generated/auth/auth.js';
	import ApiKeySettings from './ApiKeySettings.svelte';
	import BillingSettings from './BillingSettings.svelte';
	import PrivacySettings from './PrivacySettings.svelte';
	let selectedId = $state('');
	const pages = createInfiniteQuery(() => ({
		queryKey: getListSettingsWorkspacesQueryKey(),
		initialPageParam: '',
		queryFn: ({ pageParam, signal }) =>
			listSettingsWorkspaces(pageParam ? { after: pageParam } : undefined, { signal }),
		getNextPageParam: (last) => last.data.next_cursor ?? undefined,
		retry: false,
		staleTime: 0
	}));
	const workspaces = $derived(pages.data?.pages.flatMap((page) => page.data.items) ?? []);
	const workspace = $derived(workspaces.find((entry) => entry.id === selectedId));
</script>

<div class="flex flex-col gap-8">
	<section class="flex flex-col gap-4" aria-labelledby="workspace-settings-title">
		<div>
			<h2 id="workspace-settings-title" class="text-lg font-semibold">
				Account and workspace
			</h2>
			<p class="text-sm text-muted-foreground">
				Choose a workspace to manage machine access and billing. Capabilities reflect
				current membership; the server checks each action again.
			</p>
		</div>
		{#if pages.isPending}<p role="status">Loading your workspaces…</p>{:else if pages.isError}<p
				role="alert"
			>
				Your workspaces could not be loaded.
			</p>{/if}
		<Field.Field
			><Field.Label for="settings-workspace">Workspace</Field.Label><Select.Root
				type="single"
				bind:value={selectedId}
				><Select.Trigger id="settings-workspace"
					>{workspace?.name ?? 'Choose a workspace'}</Select.Trigger
				><Select.Content
					><Select.Group
						>{#each workspaces as entry (entry.id)}<Select.Item
								value={entry.id}
								label={entry.name}>{entry.name}</Select.Item
							>{/each}</Select.Group
					></Select.Content
				></Select.Root
			></Field.Field
		>
		<div class="flex flex-wrap gap-2">
			<Button
				variant="outline"
				size="sm"
				disabled={pages.isFetching}
				onclick={() => void pages.refetch()}>Refresh workspaces</Button
			>{#if pages.hasNextPage}<Button
					variant="outline"
					size="sm"
					disabled={pages.isFetching}
					onclick={() => void pages.fetchNextPage()}>Load more workspaces</Button
				>{/if}
		</div>
		{#if pages.isSuccess && workspaces.length === 0}<p class="text-sm text-muted-foreground">
				No active workspace memberships.
			</p>{/if}
	</section>
	{#if workspace && !pages.isError}
		{#key workspace.id}
			{#if workspace.can_manage_keys}<ApiKeySettings workspaceId={workspace.id} />{:else}<p
					class="text-sm text-muted-foreground"
				>
					Your current workspace role cannot manage API keys.
				</p>{/if}
			{#if workspace.can_manage_billing}<BillingSettings
					workspaceId={workspace.id}
				/>{:else}<p class="text-sm text-muted-foreground">
					Your current workspace role cannot manage billing.
				</p>{/if}
		{/key}
	{/if}
	<PrivacySettings />
</div>
