import prettier from 'eslint-config-prettier';
import pluginQuery from '@tanstack/eslint-plugin-query';
import path from 'node:path';
import js from '@eslint/js';
import svelte from 'eslint-plugin-svelte';
import { defineConfig, includeIgnoreFile } from 'eslint/config';
import globals from 'globals';
import ts from 'typescript-eslint';

const rootGitignorePath = path.resolve(import.meta.dirname, '../../.gitignore');
const webGitignorePath = path.resolve(import.meta.dirname, '.gitignore');

export default defineConfig(
	includeIgnoreFile(rootGitignorePath),
	includeIgnoreFile(webGitignorePath),
	{
		ignores: [
			'.svelte-kit/**',
			'build/**',
			'dist/**',
			'.stryker-tmp/**',
			'node_modules/**',
			'src/lib/api/generated/**'
		]
	},
	js.configs.recommended,
	ts.configs.recommended,
	svelte.configs.recommended,
	prettier,
	svelte.configs.prettier,
	pluginQuery.configs['flat/recommended-strict'],
	{
		languageOptions: { globals: { ...globals.browser, ...globals.node } },
		rules: {
			// typescript-eslint strongly recommend that you do not use the no-undef lint rule on TypeScript projects.
			// see: https://typescript-eslint.io/troubleshooting/faqs/eslint/#i-get-errors-from-the-no-undef-rule-about-global-variables-not-being-defined-even-though-there-are-no-typescript-errors
			'no-undef': 'off'
		}
	},
	{
		files: ['**/*.svelte', '**/*.svelte.ts', '**/*.svelte.js'],
		languageOptions: {
			parserOptions: {
				projectService: true,
				extraFileExtensions: ['.svelte'],
				parser: ts.parser
			}
		}
	},
	{
		files: ['**/*.svelte'],
		rules: {
			'svelte/no-at-html-tags': 'error',
			'svelte/no-target-blank': 'error'
		}
	},
	{
		files: ['src/**/*.{js,ts,svelte}'],
		rules: {
			'no-restricted-imports': [
				'error',
				{
					patterns: [
						{
							group: ['@deepref/ui/src/**', '@deepref/ui/src'],
							message:
								'Do not import directly from @deepref/ui/src. Use explicit entrypoints like @deepref/ui/button or @deepref/ui/page-header.'
						},
						{
							group: ['$lib/components', '$lib/components/**'],
							message:
								'The $lib/components layer has been retired. Use @deepref/ui for presentation, $lib/shell for shell chrome, or $lib/features for feature modules.'
						}
					]
				}
			]
		}
	},
	{
		files: ['src/lib/utils.ts'],
		rules: {
			'no-restricted-imports': [
				'error',
				{
					patterns: [
						{
							group: [
								'$lib/api',
								'$lib/api/**',
								'$lib/shell',
								'$lib/shell/**',
								'$lib/features',
								'$lib/features/**',
								'**/api/**',
								'**/shell/**',
								'**/features/**',
								'**/routes/**'
							],
							message:
								'The utils layer must not depend on api, shell, features, or routes.'
						}
					]
				}
			]
		}
	},
	{
		files: ['src/lib/api/**/*.{js,ts}'],
		rules: {
			'no-restricted-imports': [
				'error',
				{
					patterns: [
						{
							group: [
								'$lib/shell',
								'$lib/shell/**',
								'$lib/features',
								'$lib/features/**',
								'**/shell/**',
								'**/features/**',
								'**/routes/**'
							],
							message: 'The api layer must not depend on shell, features, or routes.'
						}
					]
				}
			]
		}
	},
	{
		files: ['src/lib/shell/**/*.{js,ts,svelte}'],
		rules: {
			'no-restricted-imports': [
				'error',
				{
					patterns: [
						{
							group: ['**/routes/**', '../routes/**', '../../routes/**'],
							message: 'The shell layer must not depend on routes.'
						}
					]
				}
			],
			'svelte/no-navigation-without-resolve': 'off'
		}
	},
	{
		files: ['src/lib/features/**/*.{js,ts,svelte}'],
		rules: {
			'no-restricted-imports': [
				'error',
				{
					patterns: [
						{
							group: [
								'**/routes/**',
								'../routes/**',
								'../../routes/**',
								'../../../routes/**'
							],
							message: 'The features layer must not depend on routes.'
						}
					]
				}
			]
		}
	},
	{
		files: [
			'src/lib/shell/**/*.{js,ts,svelte}',
			'src/lib/features/**/*.{js,ts,svelte}',
			'src/routes/**/*.{js,ts,svelte}'
		],
		rules: {
			'no-restricted-globals': [
				'error',
				{
					name: 'fetch',
					message:
						'Direct call to global fetch() is forbidden in UI layers. Use customFetch or API client queries from $lib/api instead.'
				}
			]
		}
	},
	{
		files: ['src/lib/features/workflows/domain/**/*.ts'],
		rules: {
			'no-restricted-imports': [
				'error',
				{
					patterns: [
						{
							group: [
								'rete',
								'rete-*',
								'rete-*/**',
								'svelte',
								'svelte/**',
								'@xyflow/**',
								'@deepref/ui',
								'@deepref/ui/**',
								'$app/**',
								'$lib/api/**',
								'**/api/**',
								'**/editor/**',
								'**/routes/**'
							],
							message:
								'Workflow definitions, validation and commands must remain renderer-neutral and independent of transport/UI modules.'
						}
					]
				}
			]
		}
	}
);
