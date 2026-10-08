<script lang="ts">
	import RefreshCwIcon from '@lucide/svelte/icons/refresh-cw';
	import TriangleAlertIcon from '@lucide/svelte/icons/triangle-alert';
	import {
		getWorkflowEndpoints,
		rotateWorkflowEmailAddress,
		rotateWorkflowWebhook,
		rotateWorkflowWebhookSecret,
		updateWorkflowEndpoints
	} from '$lib/api/generated/automations/automations';
	import type { EndpointsDto } from '$lib/api/generated/models';
	import { notifyError, notifySuccess } from '$lib/features/notifications/toast';
	import { Button } from '@deepref/ui/button';
	import { CopyButton } from '@deepref/ui/copy-button';
	import { Input } from '@deepref/ui/input';
	import { Label } from '@deepref/ui/label';
	import { Switch } from '@deepref/ui/switch';
	import { absoluteHookUrl } from './api';
	import { signatureRule, webhookCurl } from './webhook';

	let {
		projectId,
		workflowId,
		kind
	}: { projectId: string; workflowId: string; kind: 'webhook' | 'email' } = $props();

	let endpoints = $state<EndpointsDto | null>(null);
	// A copyable command that posts a sample call with a correct signature.
	const curlExample = $derived(
		endpoints
			? webhookCurl(absoluteHookUrl(endpoints.webhook_path), endpoints.signature_header)
			: ''
	);
	let failed = $state(false);
	let busy = $state(false);
	let revealSecret = $state(false);

	async function load() {
		try {
			endpoints = (await getWorkflowEndpoints(projectId, workflowId)).data;
			failed = false;
		} catch {
			failed = true;
		}
	}

	$effect(() => {
		void workflowId;
		void load();
	});

	async function rotate(which: 'url' | 'secret' | 'email') {
		busy = true;
		try {
			const call =
				which === 'url'
					? rotateWorkflowWebhook
					: which === 'secret'
						? rotateWorkflowWebhookSecret
						: rotateWorkflowEmailAddress;
			endpoints = (await call(projectId, workflowId)).data as EndpointsDto;
			notifySuccess(
				which === 'secret'
					? 'New secret created'
					: 'New address created — the old one no longer works'
			);
		} catch (error) {
			notifyError('Could not create a new one', error);
		} finally {
			busy = false;
		}
	}

	async function setSigned(required: boolean) {
		busy = true;
		try {
			endpoints = (
				await updateWorkflowEndpoints(projectId, workflowId, {
					signature_required: required
				})
			).data as EndpointsDto;
		} catch (error) {
			notifyError('Could not change this setting', error);
		} finally {
			busy = false;
		}
	}
</script>

<div
	class="flex flex-col gap-3 rounded-md border border-border bg-muted/40 p-3"
	data-testid="connection-panel"
>
	{#if failed}
		<p class="text-sm text-muted-foreground">Could not load the address. Try again later.</p>
	{:else if !endpoints}
		<p class="text-sm text-muted-foreground">Loading…</p>
	{:else if kind === 'webhook'}
		<div class="flex flex-col gap-1.5">
			<Label for="hook-url">Web address to call</Label>
			<div class="flex items-center gap-1">
				<Input id="hook-url" readonly value={absoluteHookUrl(endpoints.webhook_path)} />
				<CopyButton
					text={absoluteHookUrl(endpoints.webhook_path)}
					aria-label="Copy the address"
				/>
				<Button
					type="button"
					size="icon-sm"
					variant="ghost"
					disabled={busy}
					aria-label="Create a new address"
					title="Create a new address"
					onclick={() => rotate('url')}><RefreshCwIcon /></Button
				>
			</div>
		</div>
		<div class="flex flex-col gap-1.5">
			<Label for="hook-secret">Secret to sign calls</Label>
			<div class="flex items-center gap-1">
				<Input
					id="hook-secret"
					readonly
					type={revealSecret ? 'text' : 'password'}
					value={endpoints.webhook_secret}
				/>
				<Button
					type="button"
					size="sm"
					variant="ghost"
					onclick={() => (revealSecret = !revealSecret)}
				>
					{revealSecret ? 'Hide' : 'Show'}
				</Button>
				<CopyButton text={endpoints.webhook_secret} aria-label="Copy the secret" />
				<Button
					type="button"
					size="icon-sm"
					variant="ghost"
					disabled={busy}
					aria-label="Create a new secret"
					title="Create a new secret"
					onclick={() => rotate('secret')}><RefreshCwIcon /></Button
				>
			</div>
			<p class="text-xs text-muted-foreground" data-testid="signature-rule">
				{signatureRule(endpoints.signature_header)}
			</p>
		</div>
		<div class="flex items-center justify-between gap-3">
			<Label for="hook-signed">Only accept signed calls</Label>
			<Switch
				id="hook-signed"
				checked={endpoints.signature_required}
				disabled={busy}
				onCheckedChange={setSigned}
			/>
		</div>
		{#if !endpoints.signature_required}
			<p class="flex items-start gap-1.5 text-xs text-warning">
				<TriangleAlertIcon class="mt-px size-3.5 shrink-0" />
				Anyone who knows the address can start this automation.
			</p>
		{/if}
		<div class="flex flex-col gap-1.5">
			<p class="text-xs font-medium">Try a signed call</p>
			<div class="flex items-start gap-1">
				<pre
					class="min-w-0 flex-1 overflow-x-auto rounded-md border border-border bg-card p-2 font-mono text-2xs leading-relaxed whitespace-pre"
					data-testid="webhook-curl">{curlExample}</pre>
				<CopyButton text={curlExample} aria-label="Copy the example command" />
			</div>
			<p class="text-xs text-muted-foreground">
				Paste it into a POSIX shell (bash or zsh) and replace PASTE_THE_SECRET_HERE with the
				secret above. It sends a sample body and starts the automation with it.
			</p>
		</div>
	{:else}
		<div class="flex flex-col gap-1.5">
			<Label for="mail-url">Forwarding address</Label>
			<div class="flex items-center gap-1">
				<Input id="mail-url" readonly value={absoluteHookUrl(endpoints.email_path)} />
				<CopyButton
					text={absoluteHookUrl(endpoints.email_path)}
					aria-label="Copy the address"
				/>
				<Button
					type="button"
					size="icon-sm"
					variant="ghost"
					disabled={busy}
					aria-label="Create a new address"
					title="Create a new address"
					onclick={() => rotate('email')}><RefreshCwIcon /></Button
				>
			</div>
			<p class="text-xs text-muted-foreground">
				Point your e-mail service’s incoming-mail forwarding to this address. Every message
				it receives starts the automation.
			</p>
		</div>
	{/if}
</div>
