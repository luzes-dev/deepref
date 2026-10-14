import { beforeEach, describe, expect, it, vi } from 'vitest';
import { render, screen, waitFor, within } from '@testing-library/svelte';
import { http, HttpResponse } from 'msw';
import { tick } from 'svelte';
import type { DocumentBlockDto } from '#lib/api/generated/models/index.js';
import { server } from '../../../../tests/mocks/server';
import * as sveltekit from '#lib/tests/fake-sveltekit.svelte.js';
import FullTextScreeningTestHost from '#lib/tests/FullTextScreeningTestHost.svelte';

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
// pdf.js cannot render in jsdom; the decision flow does not need the canvas.
vi.mock('#lib/features/full-text/components/PdfViewer.svelte', async () => ({
	default: (await import('#lib/tests/StubComponent.svelte')).default
}));

const projectId = 'project-1';
const base = `/api/projects/${projectId}`;

type Report = { report_id: string; title: string; full_text_status: string; revision: number };
type Body = { decision?: string; exclusion_reason_id?: string; expected_revision: number };
type Call = { reportId: string; body: Body };

function documentFor(reportId: string) {
	return {
		id: `document-${reportId}`,
		report_id: reportId,
		mime_type: 'application/pdf',
		byte_size: 128,
		content_hash: 'sha256-fixture',
		original_filename: 'evidence.pdf',
		source: 'upload',
		status: 'available',
		ocr_required: false,
		parser_version: 'fixture-parser-v1',
		parser_error: null,
		identity: null,
		created_at: '2026-01-01T00:00:00Z',
		updated_at: '2026-01-01T00:00:00Z'
	};
}

function blockFor(reportId: string, id: string, text: string, pageNumber = 1): DocumentBlockDto {
	return {
		id,
		document_id: `document-${reportId}`,
		content_hash: 'fixture-block-hash',
		kind: 'paragraph',
		ordinal: pageNumber - 1,
		page_number: pageNumber,
		page_ocr_required: false,
		parser_version: 'fixture-parser-v1',
		section_path: ['Methods'],
		text
	};
}

function installApi(reportCount = 2) {
	const reports: Report[] = Array.from({ length: reportCount }, (_, index) => index + 1).map((index) => ({
		report_id: `report-${index}`,
		title: `Full-text report ${index}`,
		full_text_status: 'unscreened',
		revision: 4
	}));
	const history = new Map<string, Record<string, unknown>[]>();
	const decisions: Call[] = [];
	const undos: Call[] = [];
	let queueGate: Promise<void> | null = null;
	const find = (reportId: string) => reports.find((report) => report.report_id === reportId)!;
	const stateFor = (report: Report, reasonId: string | null = null) => ({
		project_id: projectId,
		report_id: report.report_id,
		title_abstract_status: 'include',
		full_text_status: report.full_text_status,
		full_text_exclusion_reason_id: reasonId,
		final_status: report.full_text_status,
		revision: report.revision,
		last_event_id: null,
		updated_at: '2026-01-01T00:00:00Z'
	});
	const event = (kind: 'decision' | 'undo', previous: string, result: string) => ({
		id: `event-${decisions.length + undos.length}`,
		event_kind: kind,
		stage: 'full_text',
		decision: kind === 'decision' ? result : null,
		notes: null,
		protocol_version_id: 'protocol-1',
		actor_kind: 'user',
		actor_id: 'reviewer',
		supersedes_event_id: null,
		undoes_event_id: null,
		created_at: '2026-01-01T00:00:00Z',
		previous_title_abstract_status: 'include',
		previous_full_text_status: previous,
		previous_full_text_exclusion_reason_id: null,
		previous_final_status: previous,
		result_title_abstract_status: 'include',
		result_full_text_status: result,
		result_full_text_exclusion_reason_id: null,
		result_final_status: result
	});

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
		http.get(`${base}/screening/full-text`, async ({ request }) => {
			if (queueGate) await queueGate;
			const url = new URL(request.url);
			const start = Number(url.searchParams.get('cursor') ?? 0);
			const limit = Number(url.searchParams.get('limit') ?? 100);
			return HttpResponse.json({
				items: reports.slice(start, start + limit).map((report) => ({
					report_id: report.report_id,
					title: report.title,
					abstract_text: null,
					doi: null,
					publication_year: 2024,
					full_text_status: report.full_text_status,
					revision: report.revision,
					document: documentFor(report.report_id)
				})),
				next_cursor: start + limit < reports.length ? String(start + limit) : null
			});
		}),
		http.get(`${base}/screening/full-text/missing`, () => HttpResponse.json([])),
		http.get(`${base}/screening/full-text/reasons`, () =>
			HttpResponse.json([
				{
					id: 'reason-0',
					code: 'wrong_population',
					label: 'Wrong population',
					stage: 'full_text'
				},
				{
					id: 'reason-1',
					code: 'wrong_outcome',
					label: 'Wrong outcome',
					stage: 'full_text'
				}
			])
		),
		http.get(`${base}/reports/:reportId/documents`, ({ params }) =>
			HttpResponse.json([documentFor(String(params.reportId))])
		),
		http.get(`${base}/reports/:reportId/documents/:documentId`, ({ params }) =>
			HttpResponse.json(documentFor(String(params.reportId)))
		),
		http.get(`${base}/reports/:reportId/documents/:documentId/blocks`, () =>
			HttpResponse.json({ items: [], next_cursor: null })
		),
		http.get(`${base}/reports/:reportId/documents/:documentId/pages`, () =>
			HttpResponse.json([])
		),
		http.get(`${base}/reports/:reportId/documents/:documentId/sections`, () =>
			HttpResponse.json([])
		),
		http.get(`${base}/reports/:reportId/documents/:documentId/references`, () =>
			HttpResponse.json([])
		),
		http.get(`${base}/reports/:reportId/screening/history`, ({ params }) =>
			HttpResponse.json({
				project_id: projectId,
				report_id: params.reportId,
				items: history.get(String(params.reportId)) ?? []
			})
		),
		http.post(`${base}/reports/:reportId/screening`, async ({ params, request }) => {
			const reportId = String(params.reportId);
			const body = (await request.json()) as Body;
			decisions.push({ reportId, body });
			const report = find(reportId);
			if (body.expected_revision !== report.revision) {
				return HttpResponse.json(
					{ code: 'screening_revision_conflict', message: 'stale screening revision' },
					{ status: 409 }
				);
			}
			const previous = report.full_text_status;
			report.full_text_status = body.decision!;
			report.revision += 1;
			history.set(reportId, [
				...(history.get(reportId) ?? []),
				event('decision', previous, report.full_text_status)
			]);
			return HttpResponse.json(stateFor(report, body.exclusion_reason_id ?? null));
		}),
		http.post(`${base}/reports/:reportId/screening/undo`, async ({ params, request }) => {
			const reportId = String(params.reportId);
			const body = (await request.json()) as Body;
			undos.push({ reportId, body });
			const report = find(reportId);
			const latest = history.get(reportId)?.at(-1);
			const previous = report.full_text_status;
			report.full_text_status = String(latest?.previous_full_text_status ?? 'unscreened');
			report.revision += 1;
			history.set(reportId, [
				...(history.get(reportId) ?? []),
				event('undo', previous, report.full_text_status)
			]);
			return HttpResponse.json(stateFor(report));
		})
	);
	return {
		decisions,
		undos,
		holdQueue() {
			let release = () => {};
			queueGate = new Promise<void>((resolve) => {
				release = () => {
					queueGate = null;
					resolve();
				};
			});
			return () => release();
		}
	};
}

function press(key: string, init: KeyboardEventInit = {}) {
	document.body.dispatchEvent(
		new KeyboardEvent('keydown', { key, bubbles: true, cancelable: true, ...init })
	);
}

const decisionBar = () => within(screen.getByTestId('full-text-decision'));

async function renderAt(report: string) {
	sveltekit.resetNavigation(`/projects/${projectId}/screening/full-text?report=${report}`);
	render(FullTextScreeningTestHost, { projectId });
	await waitFor(() =>
		expect(screen.getByRole('heading', { level: 2 })).toHaveTextContent(
			`Full-text report ${report.split('-')[1]}`
		)
	);
	await waitFor(() =>
		expect(decisionBar().getByRole('button', { name: /^Include/ })).not.toBeDisabled()
	);
}

async function settle() {
	await waitFor(() => expect(screen.queryByText('Saving…')).not.toBeInTheDocument());
	await new Promise((resolve) => setTimeout(resolve, 50));
	await waitFor(() => expect(screen.queryByText('Saving…')).not.toBeInTheDocument());
}

beforeEach(() => {
	sveltekit.stubMatchMedia();
});

describe('full-text screening shortcuts', () => {
	it('opens a cited report beyond the first queue page without changing the citation', async () => {
		installApi(101);
		server.use(
			http.get(`${base}/reports/report-101/documents/:documentId/blocks`, () =>
				HttpResponse.json({
					items: [blockFor('report-101', 'linked-passage', 'Cited late-page evidence', 4)],
					next_cursor: null
				})
			)
		);
		const search = '?report=report-101&page=4&block=linked-passage';
		sveltekit.resetNavigation(`/projects/${projectId}/screening/full-text${search}`);
		render(FullTextScreeningTestHost, { projectId });
		await waitFor(() =>
			expect(screen.getByRole('heading', { level: 2 })).toHaveTextContent('Full-text report 101')
		);
		await waitFor(() => expect(screen.getByText('Cited late-page evidence')).toBeVisible());
		expect(sveltekit.page.url.search).toBe(search);
	});
	it('retries a failed queue page while retaining the requested citation', async () => {
		installApi(101);
		let failed = false;
		server.use(
			http.get(`${base}/screening/full-text`, ({ request }) => {
				if (!new URL(request.url).searchParams.has('cursor')) return;
				if (!failed) {
					failed = true;
					return HttpResponse.json(
						{ code: 'DATABASE_UNAVAILABLE', message: 'Queue page unavailable' },
						{ status: 503 }
					);
				}
				return HttpResponse.json({
					items: [{
						report_id: 'report-101',
						title: 'Full-text report 101',
						abstract_text: null,
						doi: null,
						publication_year: 2024,
						full_text_status: 'unscreened',
						revision: 4,
						document: documentFor('report-101')
					}],
					next_cursor: null
				});
			})
		);
		const search = '?report=report-101&page=4&block=linked-passage';
		sveltekit.resetNavigation(`/projects/${projectId}/screening/full-text${search}`);
		render(FullTextScreeningTestHost, { projectId });
		await waitFor(() => expect(screen.getByRole('alert')).toHaveTextContent('Queue page unavailable'));
		screen.getByRole('button', { name: 'Retry queue' }).click();
		await waitFor(() =>
			expect(screen.getByRole('heading', { level: 2 })).toHaveTextContent('Full-text report 101')
		);
		expect(sveltekit.page.url.search).toBe(search);
	});

	it('does not replace an absent cited report with an unrelated queue item', async () => {
		const api = installApi();
		sveltekit.resetNavigation(`/projects/${projectId}/screening/full-text?report=absent&page=4&block=linked-passage`);
		render(FullTextScreeningTestHost, { projectId });
		await waitFor(() =>
			expect(screen.getByRole('heading', { level: 2 })).toHaveTextContent('Requested report is not in the full-text queue')
		);
		press('i');
		await tick();
		expect(api.decisions).toEqual([]);
		expect(sveltekit.page.url.search).toBe('?report=absent&page=4&block=linked-passage');
	});


	it('records one decision for a held key on the displayed report', async () => {
		const api = installApi();
		await renderAt('report-2');

		press('i');
		for (let index = 0; index < 4; index += 1) press('i', { repeat: true });
		await waitFor(() => expect(api.decisions.length).toBeGreaterThan(0));
		await settle();

		expect(api.decisions).toEqual([
			{
				reportId: 'report-2',
				body: expect.objectContaining({ decision: 'include', expected_revision: 4 })
			}
		]);
	});

	it('records one decision for a double press in the same tick', async () => {
		const api = installApi();
		await renderAt('report-2');

		press('m');
		press('m');
		await waitFor(() => expect(api.decisions.length).toBeGreaterThan(0));
		await settle();

		expect(api.decisions.map((call) => call.reportId)).toEqual(['report-2']);
	});

	it('ignores keys until the queue has caught up with the recorded decision', async () => {
		const api = installApi();
		await renderAt('report-2');

		const release = api.holdQueue();
		press('i');
		await vi.waitFor(() =>
			expect(screen.getByText('Full-text include recorded.')).toBeVisible()
		);
		press('m');
		release();
		await settle();

		expect(api.decisions.map((call) => call.body.decision)).toEqual(['include']);
	});

	it('records one exclusion for a held reason key', async () => {
		const api = installApi();
		await renderAt('report-1');

		press('e');
		press('e', { repeat: true });
		await waitFor(() => expect(decisionBar().getByText('Exclude because')).toBeInTheDocument());
		press('1');
		for (let index = 0; index < 3; index += 1) press('1', { repeat: true });
		await waitFor(() => expect(api.decisions.length).toBeGreaterThan(0));
		await settle();

		expect(api.decisions).toEqual([
			{
				reportId: 'report-1',
				body: expect.objectContaining({
					decision: 'exclude',
					exclusion_reason_id: 'reason-0'
				})
			}
		]);
	});

	it('records a clicked exclusion reason for the report that opened the picker', async () => {
		installApi();
		await renderAt('report-1');
		decisionBar().getByRole('button', { name: /^Exclude/ }).click();
		await waitFor(() => expect(decisionBar().getByRole('button', { name: /Wrong population/ })).toBeVisible());
		decisionBar().getByRole('button', { name: /Wrong population/ }).click();

		await waitFor(() => expect(decisionBar().getByRole('button', { name: /^Exclude/ })).toHaveAttribute('aria-pressed', 'true'));
		await waitFor(() => expect(decisionBar().getByRole('button', { name: 'Undo latest full-text decision' })).not.toBeDisabled());
	});

	it('cancels a pending exclusion when the reviewer switches reports', async () => {
		const api = installApi();
		await renderAt('report-1');
		press('e');
		await waitFor(() => expect(decisionBar().getByText('Exclude because')).toBeVisible());

		screen.getByRole('button', { name: 'Next report (ArrowRight)' }).click();
		await waitFor(() => expect(screen.getByRole('heading', { level: 2 })).toHaveTextContent('Full-text report 2'));
		await waitFor(() => expect(screen.getByRole('group', { name: 'Document view' })).toBeVisible());
		press('1');
		await tick();

		await waitFor(() => expect(decisionBar().getByRole('button', { name: /^Exclude/ })).toHaveAttribute('aria-pressed', 'false'));
		expect(api.decisions).toEqual([]);
		expect(decisionBar().queryByText('Exclude because')).not.toBeInTheDocument();
	});

	it('still undoes the last decision once', async () => {
		const api = installApi();
		await renderAt('report-1');

		press('i');
		await vi.waitFor(() =>
			expect(screen.getByText('Full-text include recorded.')).toBeVisible()
		);
		await settle();
		await waitFor(() =>
			expect(
				decisionBar().getByRole('button', { name: 'Undo latest full-text decision' })
			).not.toBeDisabled()
		);
		press('u');
		press('u', { repeat: true });
		await vi.waitFor(() =>
			expect(screen.getByText('The last full-text decision was undone.')).toBeVisible()
		);
		await settle();

		expect(api.undos).toEqual([
			{ reportId: 'report-1', body: expect.objectContaining({ expected_revision: 5 }) }
		]);
	});

	it('shows a later evidence-page failure and retries the complete evidence set', async () => {
		installApi();
		let failLaterPage = true;
		server.use(http.get(`${base}/reports/:reportId/documents/:documentId/blocks`, ({ request }) => {
			if (!new URL(request.url).searchParams.has('cursor')) {
				return HttpResponse.json({
					items: [blockFor('report-1', 'first-block', 'First page evidence')],
					next_cursor: 'later-page'
				});
			}
			return failLaterPage
				? HttpResponse.json({ code: 'DATABASE_UNAVAILABLE', message: 'Evidence service unavailable' }, { status: 503 })
				: HttpResponse.json({
					items: [blockFor('report-1', 'later-block', 'Later page evidence', 2)],
					next_cursor: null
				});
		}));
		await renderAt('report-1');
		screen.getByRole('button', { name: /Parsed evidence/ }).click();

		const retry = await screen.findByRole('button', { name: 'Retry evidence' });
		expect(screen.getByRole('alert')).toHaveTextContent('Evidence service unavailable');
		expect(screen.queryByText('First page evidence')).not.toBeInTheDocument();
		failLaterPage = false;
		retry.click();

		expect(await screen.findByText('First page evidence')).toBeVisible();
		expect(await screen.findByText('Later page evidence')).toBeVisible();
		expect(screen.queryByRole('alert')).not.toBeInTheDocument();
	});
});
