import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import type { ProjectionMetadata } from '#lib/api/generated/models/index.js';
import ProjectionFreshness from './ProjectionFreshness.svelte';

const base: ProjectionMetadata = {
	project_id: 'project-test',
	state: 'ready',
	revision: 0,
	applied_revision: 0,
	requested_revision: 0,
	watermark: 0,
	lag: 0
};

describe('projection freshness', () => {
	it('labels a successful current snapshot', () => {
		render(ProjectionFreshness, { projection: base });
		expect(screen.getByRole('status', { name: 'Metric freshness' })).toHaveTextContent(
			'Metrics current'
		);
	});
	const cases: Array<[ProjectionMetadata['state'], string]> = [
		['pending', 'Metrics pending'],
		['lagging', 'Metrics out of date'],
		['rebuilding', 'Metrics rebuilding'],
		['failed', 'Metrics refresh failed']
	];
	it.each(cases)('distinguishes %s from current even with zero lag', (state, label) => {
		render(ProjectionFreshness, {
			projection: { ...base, state, last_error: 'PRIVATE_INTERNAL_FAILURE_MARKER' }
		});
		expect(screen.getByRole('status', { name: 'Metric freshness' })).toHaveTextContent(label);
		expect(screen.queryByText('Metrics current')).not.toBeInTheDocument();
		expect(screen.queryByText(/PRIVATE_INTERNAL_FAILURE_MARKER/)).not.toBeInTheDocument();
	});
});
