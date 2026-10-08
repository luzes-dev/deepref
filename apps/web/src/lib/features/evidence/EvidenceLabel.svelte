<script lang="ts">
	import { cn } from '$lib/utils';

	/**
	 * A citation or provenance reference as readable text. The ids, hash and
	 * parser version behind it go into a tooltip, so they can still be checked
	 * without filling a phone screen. Long labels wrap instead of overflowing.
	 */
	let {
		label,
		technical,
		class: className = ''
	}: {
		label: string;
		technical?: string;
		class?: string;
	} = $props();
</script>

<span class={cn('block min-w-0 break-words', className)} title={technical || undefined}>
	{label}
</span>
