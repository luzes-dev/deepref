import { defineConfig } from 'orval';

const openapiTarget = process.env.DEEPREF_OPENAPI_TARGET ?? '../../docs/openapi.json';
const generatedDirectory = process.env.DEEPREF_GENERATED_DIR ?? 'src/lib/api/generated';
const mutatorPath = process.env.DEEPREF_MUTATOR_PATH ?? './src/lib/api/custom-fetch.ts';

export default defineConfig({
	deepref: {
		input: {
			target: openapiTarget
		},
		output: {
			mode: 'tags-split',
			target: `${generatedDirectory}/deepref.ts`,
			schemas: `${generatedDirectory}/models`,
			client: 'svelte-query',
			httpClient: 'fetch',
			baseUrl: '/api',
			clean: true,
			formatter: 'prettier',
			override: {
				fetch: {
					forceSuccessResponse: true
				},
				mutator: {
					path: mutatorPath,
					name: 'customFetch'
				},
				query: {
					usePrefetch: true,
					shouldExportQueryKey: true,
					signal: true,
					mutationInvalidates: [
						{
							onMutations: ['updateSettings'],
							invalidates: ['getSettings']
						},
						{
							onMutations: ['createProject'],
							invalidates: ['listProjects']
						},
						{
							onMutations: ['updateProject'],
							invalidates: [
								'listProjects',
								{ query: 'getProject', params: ['projectId'] }
							]
						},
						{
							onMutations: ['deleteProject'],
							invalidates: ['listProjects']
						},
						{
							onMutations: ['createIngestion'],
							invalidates: ['listIngestions']
						},
						{
							onMutations: ['cancelIngestion'],
							invalidates: [
								'listIngestions',
								{ query: 'getIngestion', params: ['ingestionId'] },
								{ query: 'listIngestionItems', params: ['ingestionId'] }
							]
						},
						{
							onMutations: ['recomputeProjectMetrics'],
							invalidates: [
								{ query: 'listProjectReports', params: ['projectId'] },
								{ query: 'getProjectGraph', params: ['projectId'] },
								{ query: 'getProjectRecommendations', params: ['projectId'] }
							]
						}
					]
				}
			}
		}
	}
});
