import { describe, it, expect } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import { http, HttpResponse } from 'msw/http';
import { server } from '../../tests/mocks/server';
import NotificationBellTestHost from '$lib/tests/NotificationBellTestHost.svelte';

const PROJECT_ID = '33438758-42f0-4da2-9c3d-22a2c51f1109';
const WORKFLOW_ID = '0c1d2e3f-0000-4000-8000-000000000001';
const RUN_ID = '0c1d2e3f-0000-4000-8000-000000000002';

function notification(overrides: Record<string, unknown> = {}) {
	return {
		id: 'aaaaaaaa-1111-1111-1111-111111111111',
		revision: 12,
		kind: 'acquisition.completed',
		severity: 'success',
		project_id: PROJECT_ID,
		project_name: 'Gap hunt - intake',
		title: 'Import completed',
		body: '24 articles imported.',
		payload: {},
		read_at: null,
		created_at: new Date().toISOString(),
		...overrides
	};
}

function unreadCount(count: number) {
	return http.get('/api/notifications/unread-count', () =>
		HttpResponse.json({ count, latest_revision: 12 })
	);
}

describe('NotificationBell', () => {
	it('shows a plain bell without unread state when the inbox is empty', async () => {
		expect.hasAssertions();
		render(NotificationBellTestHost);

		const button = await screen.findByRole('button', { name: 'Notifications' });
		expect(button).toBeInTheDocument();
		expect(screen.queryByTestId('notifications-unread-dot')).not.toBeInTheDocument();
	});

	it('switches to the ringed bell with a dot while notifications are unread', async () => {
		expect.hasAssertions();
		server.use(unreadCount(1));
		render(NotificationBellTestHost);

		const button = await screen.findByRole('button', { name: 'Notifications, 1 unread' });
		expect(screen.getByTestId('notifications-unread-dot')).toBeInTheDocument();
		expect(button).toBeInTheDocument();
	});

	it('opening the panel lists rows and leaves them unread until they are acted on', async () => {
		expect.hasAssertions();
		let markReadCount = 0;
		server.use(
			unreadCount(1),
			http.get('/api/notifications', () =>
				HttpResponse.json({ items: [notification()], next_cursor: null })
			),
			http.post('/api/notifications/mark-read', () => {
				markReadCount += 1;
				return HttpResponse.json({ updated: 1 });
			})
		);
		render(NotificationBellTestHost);

		await fireEvent.click(
			await screen.findByRole('button', { name: 'Notifications, 1 unread' })
		);

		expect(await screen.findByTestId('notifications-panel')).toBeInTheDocument();
		expect(await screen.findAllByTestId('notifications-item')).toHaveLength(1);
		expect(screen.getByText('Import completed')).toBeInTheDocument();
		expect(screen.getByText('24 articles imported.')).toBeInTheDocument();
		expect(screen.getByTestId('notifications-count')).toHaveTextContent('1 unread');
		expect(screen.getByTestId('notifications-item')).toHaveAttribute('data-read', 'false');
		expect(markReadCount).toBe(0);
	});

	it('shows the real unread total in the header, not the length of the page', async () => {
		expect.hasAssertions();
		server.use(
			unreadCount(211),
			http.get('/api/notifications', () =>
				HttpResponse.json({ items: [notification()], next_cursor: 'page-2' })
			)
		);
		render(NotificationBellTestHost);

		await fireEvent.click(
			await screen.findByRole('button', { name: 'Notifications, 211 unread' })
		);

		expect(await screen.findByTestId('notifications-count')).toHaveTextContent('211 unread');
	});

	it('marks everything read in the current project only when asked', async () => {
		expect.hasAssertions();
		const bodies: unknown[] = [];
		server.use(
			unreadCount(1),
			http.get('/api/notifications', () =>
				HttpResponse.json({ items: [notification()], next_cursor: null })
			),
			http.post('/api/notifications/mark-read', async ({ request }) => {
				bodies.push(await request.json());
				return HttpResponse.json({ updated: 1 });
			})
		);
		render(NotificationBellTestHost, { props: { projectId: PROJECT_ID } });

		await fireEvent.click(
			await screen.findByRole('button', { name: 'Notifications, 1 unread' })
		);
		await fireEvent.click(await screen.findByTestId('notifications-mark-all'));

		await waitFor(() => expect(bodies).toHaveLength(1));
		expect(bodies[0]).toEqual({ all: true, project_id: PROJECT_ID });
	});

	it('filters to the project in view and widens to every project on request', async () => {
		expect.hasAssertions();
		const projectsAsked: Array<string | null> = [];
		server.use(
			unreadCount(1),
			http.get('/api/notifications', ({ request }) => {
				projectsAsked.push(new URL(request.url).searchParams.get('project_id'));
				return HttpResponse.json({ items: [notification()], next_cursor: null });
			})
		);
		render(NotificationBellTestHost, { props: { projectId: PROJECT_ID } });

		await fireEvent.click(
			await screen.findByRole('button', { name: 'Notifications, 1 unread' })
		);
		await screen.findByTestId('notifications-item');
		expect(projectsAsked).toContain(PROJECT_ID);
		expect(screen.getByTestId('notifications-scope-project')).toHaveAttribute(
			'aria-pressed',
			'true'
		);

		await fireEvent.click(screen.getByTestId('notifications-scope-all'));
		await waitFor(() => expect(projectsAsked).toContain(null));
		expect(screen.getByTestId('notifications-scope-all')).toHaveAttribute(
			'aria-pressed',
			'true'
		);
	});

	it('labels each row with its project name', async () => {
		expect.hasAssertions();
		server.use(
			unreadCount(1),
			http.get('/api/notifications', () =>
				HttpResponse.json({ items: [notification()], next_cursor: null })
			)
		);
		render(NotificationBellTestHost);

		await fireEvent.click(
			await screen.findByRole('button', { name: 'Notifications, 1 unread' })
		);

		expect(await screen.findByTestId('notifications-project')).toHaveTextContent(
			'Gap hunt - intake'
		);
	});

	it('makes a row a link to its run and marks that row read when it is opened', async () => {
		expect.hasAssertions();
		const idsMarked: unknown[] = [];
		server.use(
			unreadCount(1),
			http.get('/api/notifications', () =>
				HttpResponse.json({
					items: [
						notification({
							id: 'bbbbbbbb-2222-2222-2222-222222222222',
							kind: 'workflow_run.failed',
							severity: 'error',
							title: 'Recent papers failed',
							body: 'That address points to a private or local network, which automations may not call.',
							payload: { workflow_id: WORKFLOW_ID, run_id: RUN_ID }
						})
					],
					next_cursor: null
				})
			),
			http.post('/api/notifications/mark-read', async ({ request }) => {
				idsMarked.push(await request.json());
				return HttpResponse.json({ updated: 1 });
			})
		);
		render(NotificationBellTestHost);

		await fireEvent.click(
			await screen.findByRole('button', { name: 'Notifications, 1 unread' })
		);
		const link = await screen.findByTestId('notifications-item-link');
		expect(link).toHaveAttribute(
			'href',
			`/projects/${PROJECT_ID}/automations/${WORKFLOW_ID}?run=${RUN_ID}`
		);

		// jsdom cannot navigate; stop the link's default action after the row handler ran.
		link.addEventListener('click', (event) => event.preventDefault());
		await fireEvent.click(link);
		await waitFor(() => expect(idsMarked).toHaveLength(1));
		expect(idsMarked[0]).toEqual({ ids: ['bbbbbbbb-2222-2222-2222-222222222222'] });
	});

	it('shows plain wording for known AI failures with the original text behind a disclosure', async () => {
		expect.hasAssertions();
		server.use(
			unreadCount(1),
			http.get('/api/notifications', () =>
				HttpResponse.json({
					items: [
						notification({
							kind: 'review_run.failed',
							severity: 'error',
							title: 'Title-abstract screening failed',
							body: 'review execution failed: AI output failed semantic validation',
							payload: { run_id: RUN_ID }
						})
					],
					next_cursor: null
				})
			)
		);
		render(NotificationBellTestHost);

		await fireEvent.click(
			await screen.findByRole('button', { name: 'Notifications, 1 unread' })
		);

		expect(
			await screen.findByText(
				'The AI could not produce a valid suggestion (it did not follow the protocol). Try again or decide yourself.'
			)
		).toBeInTheDocument();
		// The panel is portalled to the document body, outside the render container.
		const details = document.querySelector('[data-testid="notifications-item"] details');
		expect(details).not.toBeNull();
		expect(details?.textContent).toContain(
			'review execution failed: AI output failed semantic validation'
		);
	});

	it('decodes HTML entities in titles', async () => {
		expect.hasAssertions();
		server.use(
			unreadCount(1),
			http.get('/api/notifications', () =>
				HttpResponse.json({
					items: [
						notification({
							kind: 'workflow',
							title: 'Included: Lancet Diabetes &amp; Endocrinology',
							body: null
						})
					],
					next_cursor: null
				})
			)
		);
		render(NotificationBellTestHost);

		await fireEvent.click(
			await screen.findByRole('button', { name: 'Notifications, 1 unread' })
		);

		expect(
			await screen.findByText('Included: Lancet Diabetes & Endocrinology')
		).toBeInTheDocument();
	});

	it('loads the next page when asked and keeps the rows already shown', async () => {
		expect.hasAssertions();
		server.use(
			unreadCount(2),
			http.get('/api/notifications', ({ request }) => {
				const cursor = new URL(request.url).searchParams.get('cursor');
				if (cursor === 'page-2') {
					return HttpResponse.json({
						items: [
							notification({
								id: 'cccccccc-3333-3333-3333-333333333333',
								title: 'Second page row'
							})
						],
						next_cursor: null
					});
				}
				return HttpResponse.json({
					items: [notification({ title: 'First page row' })],
					next_cursor: 'page-2'
				});
			})
		);
		render(NotificationBellTestHost);

		await fireEvent.click(
			await screen.findByRole('button', { name: 'Notifications, 2 unread' })
		);
		await screen.findByText('First page row');
		await fireEvent.click(await screen.findByTestId('notifications-load-more'));

		expect(await screen.findByText('Second page row')).toBeInTheDocument();
		expect(screen.getByText('First page row')).toBeInTheDocument();
		expect(screen.queryByTestId('notifications-load-more')).not.toBeInTheDocument();
	});
});
