<script lang="ts">
	import type { StudyDto } from '$lib/api/generated/models';
	import { Checkbox } from '$lib/components/ui/checkbox';
	import * as Field from '$lib/components/ui/field';
	import { Button } from '$lib/components/ui/button';
	import * as Select from '$lib/components/ui/select';

	type DesignOption = { value: string; label: string };
	type Props = {
		study: StudyDto;
		designs: readonly DesignOption[];
		disabled: boolean;
		onSubmit: (request: {
			design: string;
			physiotherapy: boolean;
			exposure: boolean;
			prediction_or_ai: boolean;
		}) => Promise<void>;
	};

	let { study, designs, disabled, onSubmit }: Props = $props();
	let design = $state((() => study.design ?? '')());
	let physiotherapy = $state((() => study.design_context.physiotherapy)());
	let exposure = $state((() => study.design_context.exposure)());
	let predictionOrAi = $state((() => study.design_context.prediction_or_ai)());

	const selectedDesignLabel = $derived(
		designs.find((option) => option.value === design)?.label ?? 'Choose a design'
	);

	async function submit(): Promise<void> {
		if (!design) return;
		await onSubmit({
			design,
			physiotherapy,
			exposure,
			prediction_or_ai: predictionOrAi
		});
	}
</script>

<form
	class="flex flex-col gap-3"
	onsubmit={(event) => {
		event.preventDefault();
		void submit();
	}}
>
	<Field.FieldGroup>
		<Field.Field>
			<Field.FieldLabel for="study-design">Normalized design</Field.FieldLabel>
			<Select.Root type="single" bind:value={design}>
				<Select.Trigger id="study-design">{selectedDesignLabel}</Select.Trigger>
				<Select.Content>
					<Select.Group>
						{#each designs as item (item.value)}
							<Select.Item value={item.value} label={item.label}
								>{item.label}</Select.Item
							>
						{/each}
					</Select.Group>
				</Select.Content>
			</Select.Root>
		</Field.Field>
		<Field.Field orientation="horizontal">
			<Checkbox id="study-physiotherapy" bind:checked={physiotherapy} {disabled} />
			<Field.FieldLabel for="study-physiotherapy">Physiotherapy context</Field.FieldLabel>
		</Field.Field>
		<Field.Field orientation="horizontal">
			<Checkbox id="study-exposure" bind:checked={exposure} {disabled} />
			<Field.FieldLabel for="study-exposure">Exposure question</Field.FieldLabel>
		</Field.Field>
		<Field.Field orientation="horizontal">
			<Checkbox id="study-prediction-ai" bind:checked={predictionOrAi} {disabled} />
			<Field.FieldLabel for="study-prediction-ai">Prediction/AI context</Field.FieldLabel>
		</Field.Field>
	</Field.FieldGroup>
	<Button type="submit" disabled={disabled || !design}>Save classification</Button>
</form>
