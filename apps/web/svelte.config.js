import adapter from '@sveltejs/adapter-static';

const config = {
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
