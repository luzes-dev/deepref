<script lang="ts">
	import { createMutation, useQueryClient } from '@tanstack/svelte-query';
	import { onCleanup, watch } from 'runed';
	import { Button } from '@deepref/ui/button';
	import { Input } from '@deepref/ui/input';
	import { Checkbox } from '@deepref/ui/checkbox';
	import * as Field from '@deepref/ui/field';
	import {
		createApiKey,
		createListApiKeys,
		rotateApiKey,
		revokeApiKey,
		getListApiKeysQueryKey
	} from '#lib/api/generated/api-keys/api-keys.js';
	import type { ApiKeyDto } from '#lib/api/generated/models/index.js';

	let { workspaceId }: { workspaceId: string } = $props();
	const client = useQueryClient();
	const controller = new AbortController();
	let alive = true;
	let authorizationGeneration = 0;
	let name = $state('');
	let projectRead = $state(true);
	let workspaceRead = $state(false);
	let expiry = $state('');
	let secret = $state<string | null>(null);
	let message = $state('');
	let confirm = $state<{ kind: 'rotate' | 'revoke'; key: ApiKeyDto } | null>(null);
	const keys = createListApiKeys(
		() => workspaceId,
		() => ({ query: { retry: false } })
	);
	watch([() => keys.isError, () => keys.isFetching], ([failed]) => {
		if (failed) {
			// Also fence in-flight responses across repeated failed refreshes.
			authorizationGeneration++;
			secret = null;
		}
	});
	const scopes = $derived([
		...(projectRead ? ['project:view'] : []),
		...(workspaceRead ? ['workspace:view'] : [])
	]);
	type Command = { kind: 'create' } | { kind: 'rotate' | 'revoke'; id: string };
	// The mutation cache receives only a command and void. A credential never
	// enters cached mutation data, query data, notifications or browser storage.
	const change = createMutation(() => ({
		mutationFn: async (command: Command): Promise<void> => {
			const generation = authorizationGeneration;
			secret = null;
			message = '';
			if (command.kind === 'revoke') {
				await revokeApiKey(workspaceId, command.id, { signal: controller.signal });
			} else {
				const response =
					command.kind === 'create'
						? await createApiKey(
								workspaceId,
								{
									name: name.trim(),
									scopes,
									expires_at: expiry
										? new Date(`${expiry}T23:59:59Z`).toISOString()
										: null
								},
								{ signal: controller.signal }
							)
						: await rotateApiKey(workspaceId, command.id, {
								signal: controller.signal
							});
				if (alive && generation === authorizationGeneration && keys.isSuccess)
					secret = response.data.secret;
			}
			if (alive) {
				confirm = null;
				name = '';
			}
		},
		onError: () => {
			if (alive)
				message =
					'The key change failed. Refresh to check its current state before retrying.';
		},
		onSettled: () => client.invalidateQueries({ queryKey: getListApiKeysQueryKey(workspaceId) })
	}));
	onCleanup(() => {
		alive = false;
		controller.abort();
		secret = null;
	});
</script>

<section class="flex flex-col gap-4" aria-labelledby="api-key-title">
	<div>
		<h3 id="api-key-title" class="text-base font-semibold">API keys</h3>
		<p class="text-sm text-muted-foreground">
			Machine keys can read selected workspace data. Human scientific decisions and account
			management require a human session. Current membership and permissions are checked on
			every request.
		</p>
	</div>
	{#if secret && keys.isSuccess}
		<div
			class="flex flex-col gap-3 rounded-md border border-border p-4"
			role="status"
			aria-label="New API key"
		>
			<p class="text-sm font-medium">Copy this key now. It will not be shown again.</p>
			<code class="text-sm break-all" data-testid="issued-api-key">{secret}</code>
			<div class="flex flex-wrap gap-2">
				<Button
					variant="outline"
					onclick={async () => {
						try {
							await navigator.clipboard.writeText(secret ?? '');
							message = 'Key copied.';
						} catch {
							message = 'Copy failed. Select the key and copy it manually.';
						}
					}}>Copy key</Button
				>
				<Button
					variant="secondary"
					onclick={() => {
						secret = null;
						message = '';
					}}>I saved the key</Button
				>
			</div>
		</div>
	{/if}
	<form
		class="flex flex-col gap-4"
		onsubmit={(event) => {
			event.preventDefault();
			if (!change.isPending && name.trim() && scopes.length)
				change.mutate({ kind: 'create' });
		}}
	>
		<Field.Set disabled={change.isPending}>
			<Field.Legend>Create a key</Field.Legend>
			<Field.Group>
				<Field.Field
					><Field.Label for="api-key-name">Key name</Field.Label><Input
						id="api-key-name"
						bind:value={name}
						maxlength={100}
						required
						placeholder="Research integration"
					/></Field.Field
				>
				<Field.Field
					><Field.Label for="api-key-expiry">Expiry date (optional, UTC)</Field.Label
					><Input id="api-key-expiry" type="date" bind:value={expiry} /></Field.Field
				>
				<Field.Field orientation="horizontal"
					><Checkbox id="key-project-read" bind:checked={projectRead} /><Field.Label
						for="key-project-read">Read projects (project:view)</Field.Label
					></Field.Field
				>
				<Field.Field orientation="horizontal"
					><Checkbox id="key-workspace-read" bind:checked={workspaceRead} /><Field.Label
						for="key-workspace-read">Read workspace (workspace:view)</Field.Label
					></Field.Field
				>
			</Field.Group>
		</Field.Set>
		<div>
			<Button type="submit" disabled={change.isPending || !name.trim() || scopes.length === 0}
				>Create API key</Button
			>
		</div>
	</form>
	{#if message}<p role="status" class="text-sm">{message}</p>{/if}
	{#if keys.isPending}<p role="status">Loading keys…</p>
	{:else if keys.isError}<p role="alert">
			Keys could not be loaded. Your permissions may have changed.
		</p>
	{:else if keys.data}
		<ul class="flex flex-col gap-3" aria-label="Workspace API keys">
			{#each keys.data.data as key (key.id)}
				<li
					class="flex flex-wrap items-center justify-between gap-3 border-b border-border py-3"
				>
					<div class="min-w-0">
						<p class="text-sm font-medium">{key.name}</p>
						<p class="text-xs text-muted-foreground">
							{key.prefix}… · {key.scopes.join(', ')} · {key.revoked_at
								? 'Revoked'
								: key.expires_at && Date.parse(key.expires_at) <= Date.now()
									? 'Expired'
									: 'Active'}
						</p>
						<p class="text-xs text-muted-foreground">
							Expires: {key.expires_at ?? 'No expiry'} · Last used: {key.last_used_at ??
								'Never'}
						</p>
					</div>
					{#if !key.revoked_at}<div class="flex gap-2">
							<Button
								variant="outline"
								size="sm"
								disabled={change.isPending}
								onclick={() => {
									secret = null;
									confirm = { kind: 'rotate', key };
								}}>Rotate {key.name}</Button
							><Button
								variant="outline"
								size="sm"
								disabled={change.isPending}
								onclick={() => {
									secret = null;
									confirm = { kind: 'revoke', key };
								}}>Revoke {key.name}</Button
							>
						</div>{/if}
				</li>
			{/each}
		</ul>
		{#if keys.data.data.length === 0}<p class="text-sm text-muted-foreground">
				No API keys in this workspace.
			</p>{/if}
	{/if}
	{#if confirm}
		<div
			class="flex flex-col gap-3 rounded-md border border-border p-4"
			role="group"
			aria-label="Confirm key change"
		>
			<p class="text-sm">
				{confirm.kind === 'rotate'
					? `Rotate ${confirm.key.name}? The old key stops working immediately. Copy the replacement once.`
					: `Revoke ${confirm.key.name}? Integrations using it stop working immediately.`}
			</p>
			<div class="flex gap-2">
				<Button
					variant="destructive"
					disabled={change.isPending}
					onclick={() => {
						if (confirm) change.mutate({ kind: confirm.kind, id: confirm.key.id });
					}}>Confirm {confirm.kind}</Button
				><Button
					variant="outline"
					disabled={change.isPending}
					onclick={() => (confirm = null)}>Keep key</Button
				>
			</div>
		</div>
	{/if}
	<div>
		<Button
			variant="ghost"
			size="sm"
			disabled={keys.isFetching}
			onclick={() => void keys.refetch()}>Refresh keys</Button
		>
	</div>
</section>
