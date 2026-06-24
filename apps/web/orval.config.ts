import { defineConfig } from 'orval';

export default defineConfig({
	deepref: {
		input: {
			target: '../../docs/openapi.json'
		},
		output: {
			mode: 'tags-split',
			target: 'src/lib/api/generated/deepref.ts',
			schemas: 'src/lib/api/generated/models',
			client: 'svelte-query',
			httpClient: 'fetch',
			baseUrl: 'http://localhost:8080',
			clean: true,
			formatter: 'prettier',
			override: {
				fetch: {
					forceSuccessResponse: true
				},
				mutator: {
					path: './src/lib/api/custom-fetch.ts',
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
								{ query: 'listProjectArticles', params: ['projectId'] },
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
