<script lang="ts">
	import { createMutation } from '@tanstack/svelte-query';
	import { onCleanup } from 'runed';
	import { Button } from '@deepref/ui/button';
	import { Input } from '@deepref/ui/input';
	import * as Field from '@deepref/ui/field';
	import * as ToggleGroup from '@deepref/ui/toggle-group';
	import {
		createBillingCustomer,
		createBillingCheckout,
		createBillingPortal
	} from '#lib/api/generated/billing/billing.js';
	import type { BillingOffering } from '#lib/api/generated/models/index.js';

	let {
		workspaceId,
		navigate = (url) => window.location.assign(url)
	}: { workspaceId: string; navigate?: (url: string) => void } = $props();
	let email = $state('');
	let name = $state('');
	let offering = $state<BillingOffering>('primary');
	let message = $state('');
	let alive = true;
	const controller = new AbortController();
	function selectOffering(value: string) {
		if (value === 'primary' || value === 'secondary') offering = value;
	}
	const change = createMutation(() => ({
		mutationFn: async (kind: 'checkout' | 'portal'): Promise<void> => {
			message = '';
			const options = { signal: controller.signal };
			if (kind === 'checkout')
				await createBillingCustomer(
					workspaceId,
					{ email: email.trim(), name: name.trim() },
					options
				);
			const response =
				kind === 'checkout'
					? await createBillingCheckout(
							workspaceId,
							{ offering, return_path: '/settings' },
							options
						)
					: await createBillingPortal(workspaceId, { return_path: '/settings' }, options);
			// Hosted links are bearer capabilities. Consume directly, return void
			// to mutation cache, and never render/persist them in settings state.
			const url = new URL(response.data.url);
			const hosts =
				kind === 'checkout'
					? ['checkout.dodopayments.com', 'test.checkout.dodopayments.com']
					: ['customer.dodopayments.com', 'test.customer.dodopayments.com'];
			if (
				url.protocol !== 'https:' ||
				!hosts.includes(url.hostname) ||
				url.username ||
				url.password ||
				url.port ||
				url.hash
			)
				throw new Error('Invalid billing link');
			if (alive) navigate(url.href);
		},
		onError: () => {
			if (alive)
				message =
					'Billing could not be opened. Check your current billing permission and contact details, or contact support. A provider operation may need reconciliation; no payment success is assumed.';
		}
	}));
	onCleanup(() => {
		alive = false;
		controller.abort();
	});
</script>

<section class="flex flex-col gap-4" aria-labelledby="billing-title">
	<div>
		<h3 id="billing-title" class="text-base font-semibold">Billing</h3>
		<p class="text-sm text-muted-foreground">
			Dodo Payments hosts checkout and the customer portal. Prices and available offerings are
			configured by DeepRef and shown at checkout. Returning here does not confirm a payment
			or subscription.
		</p>
	</div>
	<form
		class="flex flex-col gap-4"
		onsubmit={(event) => {
			event.preventDefault();
			if (!change.isPending) change.mutate('checkout');
		}}
	>
		<Field.Set disabled={change.isPending}
			><Field.Legend>Billing contact</Field.Legend><Field.Description
				>These details are sent to Dodo for this workspace. Scientific content and member
				lists are not sent.</Field.Description
			><Field.Group>
				<Field.Field
					><Field.Label for="billing-name">Billing name</Field.Label><Input
						id="billing-name"
						bind:value={name}
						required
						maxlength={200}
						autocomplete="name"
					/></Field.Field
				>
				<Field.Field
					><Field.Label for="billing-email">Billing email</Field.Label><Input
						id="billing-email"
						type="email"
						bind:value={email}
						required
						maxlength={320}
						autocomplete="email"
					/></Field.Field
				>
				<Field.Field
					><Field.Label>Offering</Field.Label><ToggleGroup.Root
						type="single"
						value={offering}
						onValueChange={selectOffering}
						aria-label="Billing offering"
						><ToggleGroup.Item value="primary">Primary offering</ToggleGroup.Item
						><ToggleGroup.Item value="secondary">Secondary offering</ToggleGroup.Item
						></ToggleGroup.Root
					><Field.Description
						>Availability is checked by the server. Checkout displays the configured
						commercial terms.</Field.Description
					></Field.Field
				>
			</Field.Group></Field.Set
		>
		<div class="flex flex-wrap gap-2">
			<Button type="submit" disabled={change.isPending || !email.trim() || !name.trim()}
				>Continue to checkout</Button
			><Button
				type="button"
				variant="outline"
				disabled={change.isPending}
				onclick={() => change.mutate('portal')}>Open customer portal</Button
			>
		</div>
	</form>
	{#if change.isPending}<p role="status">Opening secure billing…</p>{/if}
	{#if message}<p role="alert">{message}</p>{/if}
</section>
