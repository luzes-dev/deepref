<script lang="ts">
	import { Button } from '$lib/components/ui/button';
	import CircleAlertIcon from '@lucide/svelte/icons/circle-alert';
	import ArrowLeftIcon from '@lucide/svelte/icons/arrow-left';
	import { page } from '$app/state';

	const error = $derived(page.error);
	const status = $derived(page.status);
	const message = $derived(
		typeof error === 'object' &&
			error !== null &&
			'message' in error &&
			typeof error.message === 'string'
			? error.message
			: 'The evidence workspace could not load this view.'
	);
	const pathname = $derived(page.url.pathname);
</script>

<svelte:head>
	<title>{status} · DeepRef</title>
	<meta name="description" content="DeepRef evidence workspace error." />
</svelte:head>

<div class="grid min-h-svh place-items-center bg-background px-6 py-16 text-foreground">
	<section class="w-full max-w-xl text-center" aria-labelledby="error-title">
		<div
			class="mx-auto mb-6 grid size-14 place-items-center rounded-2xl bg-destructive/10 text-destructive"
		>
			<CircleAlertIcon aria-hidden="true" />
		</div>
		<p class="text-xs font-semibold tracking-[0.18em] text-primary uppercase">
			DeepRef / {status}
		</p>
		<h1 id="error-title" class="editorial-title mt-3 text-4xl sm:text-5xl">
			A note went missing.
		</h1>
		<p class="mx-auto mt-4 max-w-md text-sm leading-relaxed text-muted-foreground">{message}</p>
		<p class="mt-3 truncate text-xs text-muted-foreground" title={pathname}>{pathname}</p>
		<div class="mt-8 flex justify-center gap-2">
			<Button href="/" variant="default">
				<ArrowLeftIcon data-icon="inline-start" aria-hidden="true" />
				Back to atelier
			</Button>
		</div>
	</section>
</div>
