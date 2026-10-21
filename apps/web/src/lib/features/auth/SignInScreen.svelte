<script lang="ts">
	import { Alert, AlertDescription, AlertTitle } from '@deepref/ui/alert';
	import { Button } from '@deepref/ui/button';
	import { Card, CardContent, CardDescription, CardHeader, CardTitle } from '@deepref/ui/card';
	import { loginUrl, returnPathFor, signInErrorMessage } from '#lib/shell/auth-urls.js';
	import type { SessionStatus } from '#lib/shell/session-status.js';

	let {
		status,
		errorCode = null,
		location
	}: {
		status: SessionStatus;
		errorCode?: string | null;
		location: { pathname: string; search: string };
	} = $props();

	const failure = $derived(signInErrorMessage(errorCode));
</script>

<main class="flex min-h-svh items-center justify-center p-6">
	<Card class="w-full max-w-sm">
		<CardHeader>
			<CardTitle>Sign in to DeepRef</CardTitle>
			<CardDescription>Continue with your DeepRef account.</CardDescription>
		</CardHeader>
		<CardContent class="flex flex-col gap-4">
			{#if status.kind === 'expired'}
				<Alert variant="warning" data-testid="session-expired">
					<AlertTitle>Your session has expired</AlertTitle>
					<AlertDescription
						>Sign in again to continue where you left off.</AlertDescription
					>
				</Alert>
			{:else if status.kind === 'error'}
				<Alert variant="destructive" data-testid="session-error">
					<AlertTitle>Could not check your session</AlertTitle>
					<AlertDescription>
						The service did not respond. Reload the page, or sign in again.
					</AlertDescription>
				</Alert>
			{/if}
			{#if failure}
				<Alert variant="destructive" data-testid="sign-in-error">
					<AlertDescription>{failure}</AlertDescription>
				</Alert>
			{/if}
			<Button href={loginUrl(returnPathFor(location))} size="lg">Sign in</Button>
		</CardContent>
	</Card>
</main>
