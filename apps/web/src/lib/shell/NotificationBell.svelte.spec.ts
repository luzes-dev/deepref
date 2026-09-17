import { describe, it, expect } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import { http, HttpResponse } from 'msw';
import { server } from '../../tests/mocks/server';
import NotificationBellTestHost from '$lib/tests/NotificationBellTestHost.svelte';

const unreadNotification = {
	id: 'aaaaaaaa-1111-1111-1111-111111111111',
	revision: 12,
	kind: 'acquisition.completed',
	severity: 'success',
	project_id: null,
	title: 'Import completed',
	body: '24 articles imported.',
	payload: {},
	read_at: null,
	created_at: new Date().toISOString()
};

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
		server.use(
			http.get('/api/notifications/unread-count', () =>
				HttpResponse.json({ count: 1, latest_revision: 12 })
			)
		);
		render(NotificationBellTestHost);

		const button = await screen.findByRole('button', { name: 'Notifications, 1 unread' });
		expect(screen.getByTestId('notifications-unread-dot')).toBeInTheDocument();
		expect(button).toBeInTheDocument();
	});

	it('lists notifications and marks them read when the panel opens', async () => {
		expect.hasAssertions();
		let markReadCount = 0;
		server.use(
			http.get('/api/notifications/unread-count', () =>
				HttpResponse.json({ count: 1, latest_revision: 12 })
			),
			http.get('/api/notifications', () =>
				HttpResponse.json({ items: [unreadNotification], next_cursor: null })
			),
			http.post('/api/notifications/mark-read', async () => {
				markReadCount += 1;
				return HttpResponse.json({ updated: 1 });
			})
		);
		render(NotificationBellTestHost);

		await fireEvent.click(
			await screen.findByRole('button', { name: 'Notifications, 1 unread' })
		);

		const panel = await screen.findByTestId('notifications-panel');
		expect(panel).toBeInTheDocument();
		const items = await screen.findAllByTestId('notifications-item');
		expect(items).toHaveLength(1);
		expect(screen.getByText('Import completed')).toBeInTheDocument();
		expect(screen.getByText('24 articles imported.')).toBeInTheDocument();
		expect(markReadCount).toBeGreaterThan(0);
	});
});
