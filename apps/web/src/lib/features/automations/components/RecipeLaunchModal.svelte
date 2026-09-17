<script lang="ts">
	import {
		configureAutomationDefinition,
		triggerAutomationManually
	} from '$lib/api/generated/automations/automations';
	import type { AutomationDefinitionDto } from '$lib/api/generated/models';
	import { ApiError } from '$lib/api/custom-fetch';
	import * as Alert from '@deepref/ui/alert';
	import { Button } from '@deepref/ui/button';
	import * as Field from '@deepref/ui/field';
	import { Input } from '@deepref/ui/input';
	import * as Modal from '@deepref/ui/modal';
	import * as Select from '@deepref/ui/select';
	import { Spinner } from '@deepref/ui/spinner';
	import { Textarea } from '@deepref/ui/textarea';
	import {
		initialRecipeParameters,
		validateRecipeParameters,
		type PredefinedRecipe
	} from '../recipes';

	let {
		projectId,
		recipe,
		open = $bindable(false),
		definitions,
		onLaunched,
		onDefinitionCreated
	}: {
		projectId: string;
		recipe: PredefinedRecipe | null;
		open: boolean;
		definitions: readonly AutomationDefinitionDto[];
		onLaunched: (runId: string) => void;
		onDefinitionCreated: () => Promise<void> | void;
	} = $props();

	const ACTOR_HEADERS = {
		'x-actor-kind': 'user',
		'x-actor-id': 'local-user'
	} satisfies Record<string, string>;

	let values = $state<Record<string, string>>({});
	let errors = $state<Record<string, string>>({});
	let editedFields = $state<Record<string, boolean>>({});
	let pending = $state(false);
	let queuedRunId = $state<string | null>(null);
	let launchError = $state<string | null>(null);

	let isLaunchable = $derived(recipe !== null);

	$effect(() => {
		if (recipe?.id) {
			values = initialRecipeParameters(recipe);
			errors = {};
			editedFields = {};
			queuedRunId = null;
			launchError = null;
		}
	});

	function dismissError(fieldKey: string): void {
		editedFields[fieldKey] = true;
	}

	function fieldError(fieldKey: string): string | undefined {
		if (editedFields[fieldKey]) return undefined;
		return errors[fieldKey];
	}

	function close(): void {
		if (pending) return;
		open = false;
	}

	async function ensureDefinition(current: PredefinedRecipe): Promise<string> {
		const existing = definitions.find(
			(definition) =>
				definition.recipe === current.backendRoute &&
				definition.trigger === 'manual' &&
				definition.status === 'active'
		);
		if (existing) return existing.id;
		const response = await configureAutomationDefinition(
			projectId,
			current.backendRoute,
			{ name: current.title, trigger: 'manual', status: 'active' },
			{ headers: ACTOR_HEADERS }
		);
		await onDefinitionCreated();
		return response.data.id;
	}

	async function submit(event: SubmitEvent): Promise<void> {
		event.preventDefault();
		const current = recipe;
		if (!current || pending) return;

		const validation = validateRecipeParameters(current, projectId, values);
		if (!validation.valid) {
			errors = validation.errors;
			editedFields = {};
			return;
		}

		pending = true;
		launchError = null;
		try {
			const definitionId = await ensureDefinition(current);
			const response = await triggerAutomationManually(
				projectId,
				{ definition_id: definitionId },
				{
					headers: {
						...ACTOR_HEADERS,
						'Idempotency-Key': crypto.randomUUID()
					}
				}
			);
			queuedRunId = response.data.run_id;
			onLaunched(response.data.run_id);
		} catch (error: unknown) {
			launchError = launchErrorMessage(error);
		} finally {
			pending = false;
		}
	}

	function launchErrorMessage(error: unknown): string {
		if (error instanceof ApiError && error.status === 409) {
			return 'The recipe is paused or changed while launching. Refresh and try again.';
		}
		if (error instanceof ApiError && error.status === 404) {
			return 'This project or recipe route no longer exists.';
		}
		return error instanceof Error ? error.message : 'The recipe run could not be queued.';
	}
</script>

<Modal.Root bind:open>
	<Modal.Content
		class="sm:max-w-lg"
		data-testid="recipe-launch-modal"
		data-recipe-id={recipe?.id ?? ''}
	>
		<Modal.Header class="border-b border-border/70 pb-4">
			<Modal.Title>{recipe?.title ?? 'Run recipe'}</Modal.Title>
			<Modal.Description>{recipe?.description}</Modal.Description>
		</Modal.Header>

		{#if recipe && queuedRunId}
			<div class="flex flex-col gap-4 px-6 py-5" data-testid="recipe-launch-success">
				<Alert.Root data-testid="recipe-launch-success-alert">
					<Alert.Title>Run queued</Alert.Title>
					<Alert.Description>
						The recipe run <code class="font-mono text-xs"
							>{queuedRunId.slice(0, 8)}</code
						>
						was queued. Track its progress under Recent activity.
					</Alert.Description>
				</Alert.Root>
			</div>
			<Modal.Footer class="border-t border-border/70 pt-4">
				<Button variant="ghost" onclick={close} data-testid="recipe-launch-close"
					>Close</Button
				>
			</Modal.Footer>
		{:else if recipe}
			<form
				class="flex flex-col gap-5 px-6 py-5"
				onsubmit={submit}
				data-testid="recipe-launch-form"
			>
				<Field.FieldGroup class="gap-5">
					{#each recipe.fields as field (field.key)}
						{@const fieldErrorText = fieldError(field.key)}
						<Field.Field data-invalid={fieldErrorText ? 'true' : undefined}>
							<Field.FieldLabel for={`recipe-field-${field.key}`}>
								{field.label}
							</Field.FieldLabel>
							{#if field.kind === 'stage'}
								<Select.Root type="single" bind:value={values[field.key] as never}>
									<Select.Trigger
										id={`recipe-field-${field.key}`}
										aria-invalid={fieldErrorText ? 'true' : undefined}
										class="w-full"
									>
										{values[field.key] === 'full_text'
											? 'Full text'
											: 'Title & abstract'}
									</Select.Trigger>
									<Select.Content>
										<Select.Item
											value="title_abstract"
											label="Title & abstract"
										/>
										<Select.Item value="full_text" label="Full text" />
									</Select.Content>
								</Select.Root>
							{:else if field.kind === 'integer'}
								<Input
									id={`recipe-field-${field.key}`}
									type="number"
									min={field.min}
									max={field.max}
									step="1"
									bind:value={values[field.key]}
									aria-invalid={fieldErrorText ? 'true' : undefined}
									oninput={() => dismissError(field.key)}
								/>
							{:else if field.kind === 'uuid-list'}
								<Textarea
									id={`recipe-field-${field.key}`}
									rows={4}
									placeholder="One UUID per line"
									bind:value={values[field.key]}
									aria-invalid={fieldErrorText ? 'true' : undefined}
									oninput={() => dismissError(field.key)}
								/>
							{:else}
								<Input
									id={`recipe-field-${field.key}`}
									type="text"
									maxlength={field.kind === 'text' ? field.maxLength : undefined}
									bind:value={values[field.key]}
									aria-invalid={fieldErrorText ? 'true' : undefined}
									oninput={() => dismissError(field.key)}
								/>
							{/if}
							{#if fieldErrorText}
								<Field.FieldError data-testid={`recipe-field-error-${field.key}`}>
									{fieldErrorText}
								</Field.FieldError>
							{:else}
								<Field.FieldDescription>{field.help}</Field.FieldDescription>
							{/if}
						</Field.Field>
					{/each}
				</Field.FieldGroup>

				{#if launchError}
					<Alert.Root
						variant="destructive"
						role="alert"
						data-testid="recipe-launch-error"
					>
						<Alert.Title>Run could not be queued</Alert.Title>
						<Alert.Description>{launchError}</Alert.Description>
					</Alert.Root>
				{/if}

				<Modal.Footer class="border-t border-border/70 pt-4">
					<Button variant="ghost" type="button" onclick={close} disabled={pending}>
						Cancel
					</Button>
					<Button
						type="submit"
						disabled={pending || !isLaunchable}
						data-testid="recipe-launch-submit"
					>
						{#if pending}<Spinner />{/if}
						Run {recipe.reviewDestination === null ? 'inspection' : 'recipe'}
					</Button>
				</Modal.Footer>
			</form>
		{/if}
	</Modal.Content>
</Modal.Root>
