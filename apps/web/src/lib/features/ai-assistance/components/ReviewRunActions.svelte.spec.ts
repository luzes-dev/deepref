import { describe, expect, it } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { http, HttpResponse } from 'msw';
import { server } from '../../../../tests/mocks/server';
import ReviewRunActionsTestHost from '#lib/tests/ReviewRunActionsTestHost.svelte';
import type { ReviewRunDto, ReviewRunStateDto } from '#lib/api/generated/models/index.js';

const projectId = '6d0f8a52-7a3b-4f43-8f4e-0b8d4a1c2e01';
const firstRunId = '6d0f8a52-7a3b-4f43-8f4e-0b8d4a1c2e02';
const nextRunId = '6d0f8a52-7a3b-4f43-8f4e-0b8d4a1c2e03';
const proposalId = '6d0f8a52-7a3b-4f43-8f4e-0b8d4a1c2e04';

function run(id: string, state: ReviewRunStateDto): ReviewRunDto {
	return {
		id,
		project_id: projectId,
		definition: 'screening',
		subject: {},
		origin: { kind: 'reviewer_requested' },
		state,
		created_at: '2026-10-11T10:00:00Z',
		started_at: state.kind === 'queued' ? null : '2026-10-11T10:00:01Z',
		finished_at:
			state.kind === 'queued' || state.kind === 'running' ? null : '2026-10-11T10:00:02Z'
	};
}

function retryEndpoint(bodies: unknown[]) {
	return http.post(
		`/api/projects/${projectId}/review-runs/${firstRunId}/retry`,
		async ({ request }) => {
			bodies.push(await request.json());
			return HttpResponse.json(run(nextRunId, { kind: 'queued' }), { status: 202 });
		}
	);
}

describe('review run retry and regenerate', () => {
	it('retries a failed run and observes the new attempt', async () => {
		const bodies: unknown[] = [];
		server.use(
			retryEndpoint(bodies),
			http.get(`/api/projects/${projectId}/review-runs/${nextRunId}`, () =>
				HttpResponse.json(run(nextRunId, { kind: 'queued' }))
			)
		);
		render(ReviewRunActionsTestHost, {
			projectId,
			run: run(firstRunId, {
				kind: 'failed',
				code: 'provider_unavailable',
				message: 'The provider timed out.'
			})
		});
		expect(await screen.findByTestId('observed-error')).toHaveTextContent(
			'The provider timed out.'
		);
		await fireEvent.click(screen.getByRole('button', { name: 'Retry' }));
		await waitFor(() => expect(bodies).toEqual([{ regenerate: false }]));
		await waitFor(() =>
			expect(screen.getByTestId('observed-run')).toHaveTextContent(`${nextRunId}:queued`)
		);
		expect(screen.getByTestId('observed-error')).toHaveTextContent('');
		expect(screen.queryByRole('button', { name: 'Retry' })).not.toBeInTheDocument();
	});

	it('offers Retry for a blocked run', async () => {
		render(ReviewRunActionsTestHost, {
			projectId,
			run: run(firstRunId, {
				kind: 'blocked',
				code: 'repair_budget_exhausted',
				message: 'Repair budget exhausted.'
			})
		});
		expect(await screen.findByRole('button', { name: 'Retry' })).toBeEnabled();
	});

	it('regenerates a completed run only when the caller allows it', async () => {
		const bodies: unknown[] = [];
		server.use(
			retryEndpoint(bodies),
			http.get(`/api/projects/${projectId}/review-runs/${nextRunId}`, () =>
				HttpResponse.json(run(nextRunId, { kind: 'queued' }))
			)
		);
		const completed = run(firstRunId, { kind: 'completed', proposal_id: proposalId });
		const { unmount } = render(ReviewRunActionsTestHost, { projectId, run: completed });
		await waitFor(() =>
			expect(screen.getByTestId('observed-run')).toHaveTextContent(`${firstRunId}:completed`)
		);
		expect(screen.queryByRole('button', { name: 'Regenerate' })).not.toBeInTheDocument();
		unmount();

		render(ReviewRunActionsTestHost, { projectId, run: completed, allowRegenerate: true });
		await fireEvent.click(await screen.findByRole('button', { name: 'Regenerate' }));
		await waitFor(() => expect(bodies).toEqual([{ regenerate: true }]));
		await waitFor(() =>
			expect(screen.getByTestId('observed-run')).toHaveTextContent(`${nextRunId}:queued`)
		);
	});
});
