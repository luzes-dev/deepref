import { loadEnv } from 'vite';
import { defineConfig } from 'vitest/config';
import tailwindcss from '@tailwindcss/vite';
import { sveltekit } from '@sveltejs/kit/vite';

export default defineConfig(({ mode }) => {
	const env = loadEnv(mode, process.cwd(), '');
	const apiProxyTarget =
		env.API_PROXY_TARGET || env.PUBLIC_API_BASE_URL || 'http://localhost:8080';

	return {
		plugins: [tailwindcss(), sveltekit()],
		server: {
			proxy: {
				'/api': {
					target: apiProxyTarget,
					changeOrigin: true,
					rewrite: (path) => path.replace(/^\/api/, '')
				}
			}
		},
		test: {
			expect: { requireAssertions: true },
			testTimeout: 20000,
			coverage: {
				provider: 'v8',
				reporter: ['text', 'json', 'html', 'lcov'],
				reportsDirectory: './coverage',
				include: ['src/**/*.{js,ts,svelte}'],
				exclude: [
					'src/**/*.d.ts',
					'src/**/*.{test,spec}.{js,ts}',
					'src/**/*.svelte.{test,spec}.{js,ts}',
					'src/**/*.e2e.ts',
					'src/lib/api/generated/**',
					'src/lib/components/ui/**',
					'**/.svelte-kit/**',
					'**/build/**',
					'**/dist/**'
				]
			},
			projects: [
				{
					extends: './vite.config.ts',
					test: {
						name: 'server',
						environment: 'node',
						include: ['src/**/*.{test,spec}.{js,ts}'],
						exclude: ['src/**/*.svelte.{test,spec}.{js,ts}']
					}
				}
			]
		}
	};
});
