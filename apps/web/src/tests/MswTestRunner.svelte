<script lang="ts">
	import TestQueryWrapper from './TestQueryWrapper.svelte';
	import MswTestFixture from './MswTestFixture.svelte';

	let { mode = 'projects' }: { mode?: 'projects' | 'settings' } = $props();
</script>

<TestQueryWrapper>
	<MswTestFixture {mode} />
</TestQueryWrapper>
