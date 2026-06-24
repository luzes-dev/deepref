<script lang="ts">
	import * as Card from '$lib/components/ui/card';
	import * as Field from '$lib/components/ui/field';
	import { Button } from '$lib/components/ui/button';
	import { Input } from '$lib/components/ui/input';
	import { Switch } from '$lib/components/ui/switch';
	import { ThemeSelector } from '$lib/components/ui/theme-selector';
	import type { UpdateSettings } from '$lib/api/generated/models';
	import { createGetSettings, createUpdateSettings } from '$lib/api/generated/settings/settings';

	const settingsQueryResult = createGetSettings();
	const updateSettings = createUpdateSettings();

	let edits = $state<Partial<UpdateSettings>>({});
	let dirty = $state(false);
	const settings = $derived({
		crossref_mailto: settingsQueryResult.data?.data.crossref_mailto ?? '',
		default_max_depth: settingsQueryResult.data?.data.default_max_depth ?? 0,
		max_concurrency: settingsQueryResult.data?.data.max_concurrency ?? 1,
		rate_limit_per_second: settingsQueryResult.data?.data.rate_limit_per_second ?? 1,
		retry_attempts: settingsQueryResult.data?.data.retry_attempts ?? 1,
		...edits
	});
	const saved = $derived(updateSettings.isSuccess && !dirty);
	const error = $derived(
		updateSettings.error?.message ?? settingsQueryResult.error?.message ?? ''
	);

	function updateSetting<Key extends keyof UpdateSettings>(key: Key, value: UpdateSettings[Key]) {
		edits = { ...edits, [key]: value };
		dirty = true;
		updateSettings.reset();
	}

	async function save() {
		try {
			await updateSettings.mutateAsync({
				data: {
					crossref_mailto: settings.crossref_mailto ?? '',
					default_max_depth: Number(settings.default_max_depth ?? 0),
					max_concurrency: Number(settings.max_concurrency ?? 1),
					rate_limit_per_second: Number(settings.rate_limit_per_second ?? 1),
					retry_attempts: Number(settings.retry_attempts ?? 1)
				}
			});
			edits = {};
			dirty = false;
		} catch {
			// Mutation state renders the API error.
		}
	}
</script>

<Card.Root class="max-w-3xl">
	<Card.Header>
		<Card.Title>Settings</Card.Title>
		<Card.Description>Application-level ingestion and Crossref parameters.</Card.Description>
	</Card.Header>
	<Card.Content>
		<Field.FieldGroup>
			<Field.Field>
				<Field.FieldLabel for="mailto">Crossref mailto</Field.FieldLabel>
				<Input
					id="mailto"
					value={settings.crossref_mailto}
					oninput={(event) => updateSetting('crossref_mailto', event.currentTarget.value)}
					placeholder="research@example.org"
				/>
				<Field.FieldDescription
					>Used in the Crossref polite pool query parameter and User-Agent.</Field.FieldDescription
				>
			</Field.Field>
			<Field.Field>
				<Field.FieldLabel for="depth">Default max depth</Field.FieldLabel>
				<Input
					id="depth"
					type="number"
					value={settings.default_max_depth}
					oninput={(event) =>
						updateSetting('default_max_depth', event.currentTarget.valueAsNumber)}
				/>
			</Field.Field>
			<Field.Field>
				<Field.FieldLabel for="concurrency">Max concurrency</Field.FieldLabel>
				<Input
					id="concurrency"
					type="number"
					value={settings.max_concurrency}
					oninput={(event) =>
						updateSetting('max_concurrency', event.currentTarget.valueAsNumber)}
				/>
			</Field.Field>
			<Field.Field>
				<Field.FieldLabel for="rate">Rate limit per second</Field.FieldLabel>
				<Input
					id="rate"
					type="number"
					value={settings.rate_limit_per_second}
					oninput={(event) =>
						updateSetting('rate_limit_per_second', event.currentTarget.valueAsNumber)}
				/>
			</Field.Field>
			<Field.Field>
				<Field.FieldLabel for="retry">Retry attempts</Field.FieldLabel>
				<Input
					id="retry"
					type="number"
					value={settings.retry_attempts}
					oninput={(event) =>
						updateSetting('retry_attempts', event.currentTarget.valueAsNumber)}
				/>
			</Field.Field>
			<Field.Field orientation="horizontal">
				<Field.FieldContent>
					<Field.FieldLabel>Dark mode</Field.FieldLabel>
					<Field.FieldDescription
						>Use the theme selector for system, light, or dark mode.</Field.FieldDescription
					>
				</Field.FieldContent>
				<ThemeSelector />
			</Field.Field>
		</Field.FieldGroup>
	</Card.Content>
	<Card.Footer class="justify-between">
		<div class="flex items-center gap-2 text-sm text-muted-foreground">
			<Switch checked={saved} />{error ||
				(updateSettings.isPending
					? 'Saving'
					: saved
						? 'Saved'
						: settingsQueryResult.isPending
							? 'Loading'
							: dirty
								? 'Unsaved changes'
								: 'Ready')}
		</div>
		<Button onclick={save} disabled={updateSettings.isPending || settingsQueryResult.isPending}
			>Save settings</Button
		>
	</Card.Footer>
</Card.Root>
