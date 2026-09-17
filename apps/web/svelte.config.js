import adapter from '@sveltejs/adapter-static';
import { vitePreprocess } from '@sveltejs/vite-plugin-svelte';

const config = {
	// The official Rete Svelte renderer ships SCSS in its presets.
	preprocess: vitePreprocess(),
	compilerOptions: {
		runes: ({ filename }) =>
			filename.split(/[/\\]/).includes('node_modules') ? undefined : true
	},
	kit: {
		adapter: adapter({
			fallback: '200.html'
		})
	}
};

export default config;
