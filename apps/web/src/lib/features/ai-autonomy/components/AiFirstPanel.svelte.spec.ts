import { afterEach, describe, expect, it } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { http, HttpResponse } from 'msw';
import { server } from '../../../../tests/mocks/server';
import AiFirstPanelTestHost from '#lib/tests/AiFirstPanelTestHost.svelte';
import type { AiFirstCohortDto, AiFirstOverviewDto } from '#lib/api/generated/models/index.js';

const projectId = '33438758-42f0-4da2-9c3d-22a2c51f1109';
const cohortId = '33438758-42f0-4da2-9c3d-22a2c51f1108';
const endpoint = `/api/projects/${projectId}/ai/first`;

function cohort(status: string, labels = 8): AiFirstCohortDto {
	return {
		id: cohortId,
		status,
		target_percent: 95,
		members: 100,
		evaluated: 90,
		quarantined: 80,
		sampled: 2,
		controls: 2,
		labels,
		reference_relevant: 12,
		alpha_billionths: 2500000,
		result:
			status === 'passed'
				? {
						observed_relevant: 0,
						first_unsafe_total: 1,
						p_value: 0.001,
						passed: true,
						reference_retention: 1
					}
				: null,
		invalidation_reason: null
	};
}

function overview(cohorts: AiFirstCohortDto[] = [], ceiling = 'routing'): void {
	const data: AiFirstOverviewDto = { ceiling, suspended_reason: null, cohorts };
	server.use(http.get(endpoint, () => HttpResponse.json(data)));
}

afterEach(() => window.sessionStorage.clear());

describe('AI-first owner controls', () => {
	it('starts with reversible routing and explicit human-reference limitations', async () => {
		overview();
		const bodies: unknown[] = [];
		server.use(
			http.post(endpoint, async ({ request }) => {
				bodies.push(await request.json());
				return HttpResponse.json({ cohort_id: cohortId, count: 0 });
			})
		);
		render(AiFirstPanelTestHost, { projectId });
		await fireEvent.click(
			await screen.findByRole('button', { name: 'Start reversible routing' })
		);
		await waitFor(() => expect(bodies).toHaveLength(1));
		expect(bodies[0]).toEqual({ target_percent: 95, allow_finalization: false });
		expect(screen.getByText(/passing does not guarantee true recall/)).toBeInTheDocument();
		expect(screen.getByText(/this is not authentication/)).toBeInTheDocument();
		expect(screen.getByText(/Later imports need a new cohort/)).toBeInTheDocument();
		expect(
			screen.getByText(/A new positive on a previously excluded control ends this/)
		).toBeInTheDocument();
	});

	it('requires two labels for every sample and interleaved control', async () => {
		overview([cohort('auditing', 4)]);
		render(AiFirstPanelTestHost, { projectId });
		expect(await screen.findByRole('button', { name: 'Evaluate once' })).toBeDisabled();
		expect(screen.getByText(/4 \/ 8 independent labels/)).toBeInTheDocument();
		expect(screen.getByText('Completed AI runs: 90 / 100')).toBeInTheDocument();
		expect(screen.queryByText('report_id')).not.toBeInTheDocument();
	});

	it('requires a separate acknowledgement before finalizing a passing cohort', async () => {
		overview([cohort('passed')], 'cohort_finalization');
		const bodies: unknown[] = [];
		server.use(
			http.post(`${endpoint}/${cohortId}/finalize`, async ({ request }) => {
				bodies.push(await request.json());
				return HttpResponse.json({ cohort_id: cohortId, count: 78 });
			})
		);
		render(AiFirstPanelTestHost, { projectId });
		const approval = await screen.findByRole('button', {
			name: 'Approve automated exclusions'
		});
		expect(approval).toBeDisabled();
		await fireEvent.click(screen.getByRole('checkbox', { name: /I approve this cohort/ }));
		expect(approval).toBeEnabled();
		await fireEvent.click(approval);
		await waitFor(() => expect(bodies).toEqual([{ acknowledge_reference_limits: true }]));
	});

	it('keeps finalization unavailable under the routing ceiling', async () => {
		overview([cohort('passed')]);
		render(AiFirstPanelTestHost, { projectId });
		const approval = await screen.findByRole('button', {
			name: 'Approve automated exclusions'
		});
		await fireEvent.click(screen.getByRole('checkbox', { name: /I approve this cohort/ }));
		expect(approval).toBeDisabled();
	});

	it('sends subsequent queue reads under the saved reviewer ID', async () => {
		const actors: (string | null)[] = [];
		server.use(
			http.get(endpoint, ({ request }) => {
				actors.push(request.headers.get('x-actor-id'));
				return HttpResponse.json({ ceiling: 'off', suspended_reason: null, cohorts: [] });
			})
		);
		render(AiFirstPanelTestHost, { projectId });
		await screen.findByRole('button', { name: 'Start reversible routing' });
		await fireEvent.input(screen.getByLabelText('Current reviewer ID'), {
			target: { value: 'reviewer-b' }
		});
		await fireEvent.click(screen.getByRole('button', { name: 'Use ID' }));
		await waitFor(() => expect(actors).toContain('reviewer-b'));
	});

	it('shows conditional audit workload and draws the chosen fixed sample', async () => {
		const closed = cohort('closed', 0);
		closed.forecast = {
			human_records_remaining: 0,
			minimum_sample: 30,
			minimum_controls: 20,
			human_judgments_if_passed: 120,
			human_judgments_if_failed: 170,
			savings_vs_single: -20,
			recommended: false
		};
		overview([closed]);
		const bodies: unknown[] = [];
		server.use(
			http.post(`${endpoint}/${cohortId}/draw`, async ({ request }) => {
				bodies.push(await request.json());
				return HttpResponse.json({ cohort_id: cohortId, count: 40 });
			})
		);
		render(AiFirstPanelTestHost, { projectId });
		expect(
			await screen.findByText('Savings below the default adoption threshold')
		).toBeInTheDocument();
		expect(screen.getByText(/omit administration and model costs/)).toBeInTheDocument();
		await fireEvent.input(screen.getByLabelText('Audit sample size (optional)'), {
			target: { value: '40' }
		});
		expect(
			screen.getByText(/Estimated human judgments: 140 if passed, 180 if failed/)
		).toBeInTheDocument();
		expect(
			screen.getByText(/40 more human judgments than single-human screening/)
		).toBeInTheDocument();
		await fireEvent.click(screen.getByRole('button', { name: 'Draw fixed audit' }));
		await waitFor(() => expect(bodies).toEqual([{ sample_size: 40 }]));
	});

	it('updates adoption advice and negative savings for a larger fixed sample', async () => {
		const closed = {
			...cohort('closed', 0),
			members: 2000,
			evaluated: 1800,
			quarantined: 1500
		};
		closed.forecast = {
			human_records_remaining: 0,
			minimum_sample: 100,
			minimum_controls: 100,
			human_judgments_if_passed: 900,
			human_judgments_if_failed: 2300,
			savings_vs_single: 1100,
			recommended: true
		};
		overview([closed]);
		render(AiFirstPanelTestHost, { projectId });
		expect(await screen.findByText('Estimated workload saving')).toBeInTheDocument();
		await fireEvent.input(screen.getByLabelText('Audit sample size (optional)'), {
			target: { value: '800' }
		});
		expect(
			screen.getByText('Savings below the default adoption threshold')
		).toBeInTheDocument();
		expect(
			screen.getByText(/Estimated human judgments: 3100 if passed, 3800 if failed/)
		).toBeInTheDocument();
		expect(
			screen.getByText(/1100 more human judgments than single-human screening/)
		).toBeInTheDocument();
	});

	it('refuses a sample below the planning baseline before sending a draw', async () => {
		const closed = cohort('closed', 0);
		closed.forecast = {
			human_records_remaining: 0,
			minimum_sample: 30,
			minimum_controls: 20,
			human_judgments_if_passed: 120,
			human_judgments_if_failed: 170,
			savings_vs_single: -20,
			recommended: false
		};
		overview([closed]);
		render(AiFirstPanelTestHost, { projectId });
		const sample = await screen.findByLabelText('Audit sample size (optional)');
		await fireEvent.input(sample, { target: { value: '20' } });
		await fireEvent.click(screen.getByRole('button', { name: 'Draw fixed audit' }));
		expect(screen.getByText(/Choose a whole number from 30 to 80/)).toBeInTheDocument();
	});

	it('allows a fresh explicit routing approval after safety suspension', async () => {
		server.use(
			http.get(endpoint, () =>
				HttpResponse.json({
					ceiling: 'routing',
					suspended_reason: 'audit_failed',
					cohorts: [cohort('failed')]
				})
			)
		);
		render(AiFirstPanelTestHost, { projectId });
		expect(
			await screen.findByRole('button', { name: 'Start reversible routing' })
		).toBeEnabled();
		expect(screen.getByText(/fresh routing approval/)).toBeInTheDocument();
	});

	it('refreshes demoted state when a refused evaluation discovers a control positive', async () => {
		let demoted = false;
		server.use(
			http.get(endpoint, () =>
				HttpResponse.json({
					ceiling: demoted ? 'off' : 'routing',
					suspended_reason: demoted ? 'human_reference_changed' : null,
					cohorts: [
						{
							...cohort(demoted ? 'invalidated' : 'auditing'),
							invalidation_reason: demoted ? 'human_reference_changed' : null
						}
					]
				})
			),
			http.post(`${endpoint}/${cohortId}/evaluate`, () => {
				demoted = true;
				return HttpResponse.json(
					{
						code: 'ai_first_refused',
						message: 'Reference positive discovered',
						details: null
					},
					{ status: 409 }
				);
			})
		);
		render(AiFirstPanelTestHost, { projectId });
		await fireEvent.click(await screen.findByRole('button', { name: 'Evaluate once' }));
		expect(await screen.findByText('Owner ceiling: off')).toBeInTheDocument();
		expect(screen.getByText('invalidated')).toBeInTheDocument();
	});

	it('refreshes the owner ceiling after recovery turns routing off', async () => {
		let recovered = false;
		server.use(
			http.get(endpoint, () =>
				HttpResponse.json({
					ceiling: recovered ? 'off' : 'routing',
					suspended_reason: null,
					cohorts: [cohort('failed')]
				})
			),
			http.post(`${endpoint}/${cohortId}/recover`, () => {
				recovered = true;
				return HttpResponse.json({ cohort_id: cohortId, count: 0 });
			})
		);
		render(AiFirstPanelTestHost, { projectId });
		await fireEvent.click(
			await screen.findByRole('button', { name: 'Discard and return to human screening' })
		);
		expect(await screen.findByText('Owner ceiling: off')).toBeInTheDocument();
	});

	it('offers recovery for already finalized exclusions', async () => {
		overview([cohort('finalized')]);
		render(AiFirstPanelTestHost, { projectId });
		expect(
			await screen.findByRole('button', { name: 'Recover and reopen exclusions' })
		).toBeEnabled();
		expect(
			screen.queryByRole('button', { name: 'Approve automated exclusions' })
		).not.toBeInTheDocument();
	});
});
