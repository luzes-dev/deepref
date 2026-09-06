<script lang="ts">
	import { createQuery, createMutation } from '@tanstack/svelte-query';

	class NetworkError extends Error {
		status?: number;
		errors?: Record<string, string[]>;
	}

	interface SettingsData {
		crossref_mailto: string;
		default_max_depth: number;
		max_concurrency: number;
	}

	interface ProjectsData {
		data: Array<{ id: string; name: string }>;
		total?: number;
		degraded?: boolean;
		degraded_reasons?: string[];
	}

	let { mode = 'projects' }: { mode?: 'projects' | 'settings' } = $props();

	const projectsQuery = createQuery<ProjectsData, NetworkError>(() => ({
		queryKey: ['msw-projects'],
		queryFn: async () => {
			const res = await fetch('/api/projects');
			if (!res.ok) {
				const errorBody = (await res.json().catch(() => ({}))) as { detail?: string };
				const error = new NetworkError(errorBody.detail || `HTTP ${res.status}`);
				error.status = res.status;
				throw error;
			}
			return res.json();
		},
		enabled: mode === 'projects',
		retry: false
	}));

	const settingsQuery = createQuery<{ data: SettingsData }, NetworkError>(() => ({
		queryKey: ['msw-settings'],
		queryFn: async () => {
			const res = await fetch('/api/settings');
			if (!res.ok) {
				const errorBody = (await res.json().catch(() => ({}))) as { detail?: string };
				const error = new NetworkError(errorBody.detail || `HTTP ${res.status}`);
				error.status = res.status;
				throw error;
			}
			return res.json();
		},
		enabled: mode === 'settings',
		retry: false
	}));

	const saveSettings = createMutation<unknown, NetworkError, Record<string, unknown>>(() => ({
		mutationFn: async (payload: Record<string, unknown>) => {
			const res = await fetch('/api/settings', {
				method: 'POST',
				headers: { 'Content-Type': 'application/json' },
				body: JSON.stringify(payload)
			});
			if (!res.ok) {
				const err = (await res.json().catch(() => ({}))) as {
					detail?: string;
					errors?: Record<string, string[]>;
				};
				const error = new NetworkError(err.detail || 'Save failed');
				error.status = res.status;
				error.errors = err.errors;
				throw error;
			}
			return res.json();
		}
	}));

	let saveInput = $state('');

	function submit() {
		saveSettings.mutate({ mailto: saveInput });
	}
</script>

<div data-testid="msw-fixture">
	{#if mode === 'projects'}
		{#if projectsQuery.isLoading}
			<p data-testid="status-loading">Loading projects...</p>
		{:else if projectsQuery.isError}
			<p data-testid="status-error">
				Error: {projectsQuery.error.message} (status: {projectsQuery.error.status})
			</p>
		{:else if projectsQuery.data}
			{#if projectsQuery.data.degraded}
				<p data-testid="status-degraded">Operating in degraded mode</p>
			{/if}
			{#if !projectsQuery.data.data || projectsQuery.data.data.length === 0}
				<p data-testid="status-empty">No projects found</p>
			{:else}
				<ul data-testid="projects-list">
					{#each projectsQuery.data.data as item (item.id)}
						<li data-testid="project-item">{item.name}</li>
					{/each}
				</ul>
			{/if}
		{/if}
	{:else}
		{#if settingsQuery.isLoading}
			<p data-testid="status-loading">Loading settings...</p>
		{:else if settingsQuery.data}
			<p data-testid="settings-mailto">{settingsQuery.data.data.crossref_mailto}</p>
		{/if}

		<input data-testid="settings-input" bind:value={saveInput} />
		<button data-testid="save-btn" onclick={submit}>Save</button>

		{#if saveSettings.isError}
			<p data-testid="mutation-error">
				Validation failed: {saveSettings.error.message}
			</p>
		{/if}
	{/if}
</div>
