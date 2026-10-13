import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/svelte';
import { http, HttpResponse } from 'msw';
import { server } from '../../../../tests/mocks/server';
import * as sveltekit from '#lib/tests/fake-sveltekit.svelte.js';
import ScreeningFocusTestHost from '#lib/tests/ScreeningFocusTestHost.svelte';

vi.mock('$app/state', async () => {
	const fake = await import('#lib/tests/fake-sveltekit.svelte.js');
	return { page: fake.page };
});
vi.mock('$app/navigation', async () => {
	const fake = await import('#lib/tests/fake-sveltekit.svelte.js');
	return { goto: fake.goto };
});
vi.mock('$app/paths', async () => {
	const fake = await import('#lib/tests/fake-sveltekit.svelte.js');
	return { resolve: fake.resolve };
});

const projectId = 'project-1';
const base = `/api/projects/${projectId}`;

type Report = {
	report_id: string;
	title: string;
	abstract_text: string;
	doi: string | null;
	publication_year: number;
	title_abstract_status: string;
	full_text_status: string;
	final_status: string;
	revision: number;
};

type Call = { reportId: string; body: { decision?: string; expected_revision: number } };

function stateFor(report: Report) {
	return {
		project_id: projectId,
		report_id: report.report_id,
		title_abstract_status: report.title_abstract_status,
		full_text_status: report.full_text_status,
		full_text_exclusion_reason_id: null,
		final_status: report.final_status,
		revision: report.revision,
		last_event_id: null,
		updated_at: '2026-01-01T00:00:00Z'
	};
}

function installApi() {
	const reports: Report[] = Array.from({ length: 4 }, (_, index) => ({
		report_id: `report-${index + 1}`,
		title: `Screening report ${index + 1}`,
		abstract_text: `Abstract ${index + 1}.`,
		doi: null,
		publication_year: 2024,
		title_abstract_status: 'unscreened',
		full_text_status: 'not_required',
		final_status: 'unscreened',
		revision: 0
	}));
	const decisions: Call[] = [];
	const undos: Call[] = [];
	const priorStatus = new Map<string, string>();
	const find = (reportId: string) => reports.find((report) => report.report_id === reportId)!;

	server.use(
		http.get('/api/ai/status', () =>
			HttpResponse.json({
				assistant_available: false,
				configured: false,
				model: null,
				provider: null,
				suggestions_available: false
			})
		),
		http.get(`${base}/protocol`, () =>
			HttpResponse.json({
				id: 'protocol-1',
				project_id: projectId,
				version: 1,
				revision: 1,
				name: 'Protocol',
				objective: 'Objective',
				question: 'Question',
				framework_kind: 'pico',
				framework_fields: {},
				status: 'published',
				criteria: [],
				created_at: '2026-01-01T00:00:00Z',
				updated_at: '2026-01-01T00:00:00Z',
				published_at: '2026-01-01T00:00:00Z',
				amendment_of: null
			})
		),
		http.get(`${base}/screening`, ({ request }) => {
			const status = new URL(request.url).searchParams.get('status') ?? 'unscreened';
			const items = reports.filter(
				(report) => status === 'all' || report.title_abstract_status === status
			);
			const unscreened = reports.filter(
				(report) => report.title_abstract_status === 'unscreened'
			).length;
			return HttpResponse.json({
				items,
				status,
				sort: 'created_asc',
				total: items.length,
				next_cursor: null,
				progress: {
					total: reports.length,
					screened: reports.length - unscreened,
					unscreened,
					included: reports.filter((report) => report.title_abstract_status === 'include')
						.length,
					excluded: reports.filter((report) => report.title_abstract_status === 'exclude')
						.length,
					maybe: reports.filter((report) => report.title_abstract_status === 'maybe')
						.length
				}
			});
		}),
		http.get(`${base}/reports/:reportId/screening/history`, ({ params }) =>
			HttpResponse.json({ project_id: projectId, report_id: params.reportId, items: [] })
		),
		http.post(`${base}/reports/:reportId/screening`, async ({ params, request }) => {
			const reportId = String(params.reportId);
			const body = (await request.json()) as Call['body'];
			decisions.push({ reportId, body });
			const report = find(reportId);
			if (body.expected_revision !== report.revision) {
				return HttpResponse.json(
					{
						code: 'screening_revision_conflict',
						message: 'stale screening revision',
						details: { currentRevision: report.revision }
					},
					{ status: 409 }
				);
			}
			priorStatus.set(reportId, report.title_abstract_status);
			report.title_abstract_status = body.decision!;
			report.final_status = body.decision!;
			report.revision += 1;
			return HttpResponse.json(stateFor(report));
		}),
		http.post(`${base}/reports/:reportId/screening/undo`, async ({ params, request }) => {
			const reportId = String(params.reportId);
			const body = (await request.json()) as Call['body'];
			undos.push({ reportId, body });
			const report = find(reportId);
			report.title_abstract_status = priorStatus.get(reportId) ?? 'unscreened';
			report.final_status = report.title_abstract_status;
			report.revision += 1;
			return HttpResponse.json(stateFor(report));
		})
	);
	return { decisions, undos };
}

function press(key: string, init: KeyboardEventInit = {}) {
	document.body.dispatchEvent(
		new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...init })
	);
}

const heading = () => screen.getByRole('heading', { level: 2 }).textContent?.trim();

async function renderAt(report: string) {
	sveltekit.resetNavigation(`/projects/${projectId}/screening/title-abstract?report=${report}`);
	render(ScreeningFocusTestHost, { projectId });
	await waitFor(() => expect(heading()).toBe(`Screening report ${report.split('-')[1]}`));
	await waitFor(() =>
		expect(
			within(screen.getByTestId('screening-decision')).getByRole('button', {
				name: /^Include/
			})
		).not.toBeDisabled()
	);
}

async function settle(reportId: string) {
	await waitFor(() => expect(sveltekit.page.url.searchParams.get('report')).toBe(reportId));
	await waitFor(() => expect(screen.queryByText('Saving…')).not.toBeInTheDocument());
	await new Promise((resolve) => setTimeout(resolve, 50));
}

beforeEach(() => {
	sveltekit.stubMatchMedia();
});

describe('title/abstract screening shortcuts', () => {
	it('records one decision for a held key on the displayed report', async () => {
		const api = installApi();
		await renderAt('report-3');

		press('i');
		for (let index = 0; index < 4; index += 1) press('i', { repeat: true });
		await settle('report-4');

		expect(api.decisions).toEqual([
			{
				reportId: 'report-3',
				body: expect.objectContaining({ decision: 'include', expected_revision: 0 })
			}
		]);
		expect(heading()).toBe('Screening report 4');
	});

	it('records one decision for a double press in the same tick', async () => {
		const api = installApi();
		await renderAt('report-3');

		press('e');
		press('e');
		await settle('report-4');

		expect(api.decisions.map((call) => call.reportId)).toEqual(['report-3']);
	});

	it('shows the next report, not the first, and ignores keys until navigation settles', async () => {
		const api = installApi();
		await renderAt('report-3');

		const release = sveltekit.holdNavigation();
		press('i');
		// The URL still names the removed report: the precomputed next item is shown.
		await waitFor(() => expect(heading()).toBe('Screening report 4'));
		expect(sveltekit.page.url.searchParams.get('report')).toBe('report-3');
		press('i');
		press('m');
		release();
		await settle('report-4');

		expect(api.decisions.map((call) => call.reportId)).toEqual(['report-3']);
		expect(heading()).toBe('Screening report 4');
	});

	it('still undoes the decision and lets the next one through', async () => {
		const api = installApi();
		await renderAt('report-3');

		press('i');
		await settle('report-4');
		press('u');
		press('u', { repeat: true });
		await settle('report-3');

		expect(api.undos).toEqual([
			{ reportId: 'report-3', body: expect.objectContaining({ expected_revision: 1 }) }
		]);
		await waitFor(() => expect(heading()).toBe('Screening report 3'));

		press('m');
		await settle('report-4');
		expect(api.decisions.map((call) => [call.reportId, call.body.decision])).toEqual([
			['report-3', 'include'],
			['report-3', 'maybe']
		]);
	});

	it('never stands in the first report for a URL report missing from the queue', async () => {
		const api = installApi();
		sveltekit.resetNavigation(
			`/projects/${projectId}/screening/title-abstract?report=report-9`
		);
		render(ScreeningFocusTestHost, { projectId });
		await waitFor(() => expect(heading()).toBe('This report is not in this view'));

		press('i');
		await new Promise((resolve) => setTimeout(resolve, 50));

		expect(api.decisions).toEqual([]);
		expect(screen.queryByTestId('screening-decision')).not.toBeInTheDocument();
	});

	it('keeps auto-repeat for arrow navigation', async () => {
		installApi();
		await renderAt('report-1');

		press('ArrowRight');
		await waitFor(() => expect(heading()).toBe('Screening report 2'));
		press('ArrowRight', { repeat: true });
		await waitFor(() => expect(heading()).toBe('Screening report 3'));
	});
});
