import { defineConfig } from 'jsrepo';

export default defineConfig({
	registries: ['@ieedan/shadcn-svelte-extras'],
	paths: {
		ui: '$lib/shell',
		component: '$lib/shell',
		block: '$lib/shell',
		hook: '$lib/hooks',
		action: '$lib/actions',
		util: '$lib/utils',
		lib: '$lib'
	}
});
