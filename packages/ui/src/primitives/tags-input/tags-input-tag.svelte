<script lang="ts">
	import XIcon from '@lucide/svelte/icons/x';

	type Props = {
		value: string;
		disabled: boolean | null;
		active: boolean;
		onDelete: (value: string) => void;
	};

	let { value, disabled, onDelete, active }: Props = $props();
</script>

<div
	class="flex place-items-center gap-2 rounded-md bg-secondary px-2 py-0.5 ring-offset-background transition-all hover:cursor-default hover:bg-secondary/90 data-[active=true]:bg-secondary/90 data-[active=true]:ring-2 data-[active=true]:ring-ring data-[active=true]:ring-offset-2"
	data-active={active}
>
	<span>
		{value}
	</span>
	<button class="flex size-6 items-center justify-center rounded-sm focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring" aria-label={`Remove ${value}`} type="button" {disabled} onclick={() => onDelete(value)}>
		<XIcon class="size-4" />
	</button>
</div>
