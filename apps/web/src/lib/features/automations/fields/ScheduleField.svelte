<script lang="ts">
	import { Button } from '@deepref/ui/button';
	import { Input } from '@deepref/ui/input';
	import { describeSchedule, isScheduleValue, WEEKDAYS, type ScheduleValue } from '../model';
	import OptionSelect from './OptionSelect.svelte';

	let { value, onchange }: { value: unknown; onchange: (value: ScheduleValue) => void } =
		$props();

	const schedule = $derived<ScheduleValue>(
		isScheduleValue(value) ? value : { every: 'day', at: '09:00', timezone: 'UTC' }
	);
	const browserZone = Intl.DateTimeFormat().resolvedOptions().timeZone || 'UTC';
	const zones = $derived.by(() => {
		const known = [
			'UTC',
			browserZone,
			'Europe/Lisbon',
			'Europe/London',
			'America/New_York',
			'America/Sao_Paulo',
			'Asia/Tokyo'
		];
		const all = new Set([schedule.timezone ?? 'UTC', ...known]);
		return [...all].map((zone) => ({
			value: zone,
			label: zone === browserZone ? `${zone} (your time zone)` : zone
		}));
	});

	function patch(change: Partial<ScheduleValue>) {
		onchange({ ...schedule, ...change });
	}
	function toggleDay(day: string) {
		const current = schedule.weekdays?.length ? schedule.weekdays : ['monday'];
		const next = current.includes(day)
			? current.filter((item) => item !== day)
			: [...current, day];
		patch({ weekdays: next.length ? next : current });
	}
	const days = $derived(schedule.weekdays?.length ? schedule.weekdays : ['monday']);
</script>

<div class="flex flex-col gap-3" data-testid="schedule-field">
	<div class="grid grid-cols-2 gap-2">
		<OptionSelect
			label="How often"
			value={schedule.every}
			options={[
				{ value: 'day', label: 'Every day' },
				{ value: 'week', label: 'Every week' },
				{ value: 'month', label: 'Every month' }
			]}
			onchange={(every) => patch({ every: every as ScheduleValue['every'] })}
		/>
		<Input
			type="time"
			aria-label="Time of day"
			value={schedule.at}
			onchange={(event) => patch({ at: event.currentTarget.value || '09:00' })}
		/>
	</div>
	{#if schedule.every === 'week'}
		<div class="flex flex-wrap gap-1" role="group" aria-label="Days of the week">
			{#each WEEKDAYS as day (day)}
				<Button
					type="button"
					size="xs"
					variant={days.includes(day) ? 'default' : 'outline'}
					aria-pressed={days.includes(day)}
					class="capitalize"
					onclick={() => toggleDay(day)}>{day.slice(0, 3)}</Button
				>
			{/each}
		</div>
	{:else if schedule.every === 'month'}
		<label class="flex items-center gap-2 text-sm">
			On day
			<Input
				type="number"
				min={1}
				max={31}
				class="w-20"
				value={schedule.day_of_month ?? 1}
				onchange={(event) =>
					patch({
						day_of_month: Math.min(
							31,
							Math.max(1, Number(event.currentTarget.value) || 1)
						)
					})}
			/>
			of the month
		</label>
	{/if}
	<OptionSelect
		label="Time zone"
		value={schedule.timezone ?? 'UTC'}
		options={zones}
		onchange={(timezone) => patch({ timezone })}
	/>
	<p class="text-xs text-muted-foreground">{describeSchedule(schedule)}</p>
</div>
