import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';

const STORAGE_KEY = 'deepref:notifications:last-seen-revision';

async function freshModule() {
	vi.resetModules();
	return await import('./state.svelte');
}

function notification(
	revision: number,
	overrides: Partial<{ read_at: string | null; created_at: string }> = {}
) {
	return {
		id: `id-${revision}`,
		revision,
		kind: 'acquisition.completed',
		severity: 'success',
		project_id: null,
		title: `Notification ${revision}`,
		body: 'done',
		payload: {},
		read_at: null,
		created_at: new Date().toISOString(),
		...overrides
	};
}

describe('notifications state', () => {
	beforeEach(() => {
		localStorage.clear();
		vi.useFakeTimers();
		vi.setSystemTime(new Date('2026-09-12T12:00:00Z'));
	});

	afterEach(() => {
		vi.useRealTimers();
	});

	it('silences history on first observation and persists the watermark', async () => {
		const { observeNotifications } = await freshModule();

		const arrivals = observeNotifications([notification(7), notification(9)]);
		expect(arrivals).toEqual([]);
		expect(localStorage.getItem(STORAGE_KEY)).toBe('9');
	});

	it('reports unread recent rows newer than the watermark as arrivals', async () => {
		localStorage.setItem(STORAGE_KEY, '9');
		const { observeNotifications } = await freshModule();

		const old = notification(10, {
			created_at: new Date(Date.now() - 60 * 60 * 1000).toISOString()
		});
		const read = notification(11, { read_at: new Date().toISOString() });
		const fresh = notification(12);

		const arrivals = observeNotifications([fresh, read, old]);
		expect(arrivals).toHaveLength(1);
		expect(arrivals[0].revision).toBe(12);
		expect(localStorage.getItem(STORAGE_KEY)).toBe('12');
	});

	it('keeps the watermark when nothing new arrives', async () => {
		localStorage.setItem(STORAGE_KEY, '12');
		const { observeNotifications } = await freshModule();

		expect(observeNotifications([notification(12)])).toEqual([]);
		expect(localStorage.getItem(STORAGE_KEY)).toBe('12');
	});

	it('formats relative timestamps', async () => {
		const { relativeTime } = await freshModule();
		const now = Date.now();

		expect(relativeTime(new Date(now - 30_000).toISOString(), now)).toBe('30 seconds ago');
		expect(relativeTime(new Date(now - 5 * 60_000).toISOString(), now)).toBe('5 minutes ago');
		expect(relativeTime(new Date(now - 2 * 3_600_000).toISOString(), now)).toBe('2 hours ago');
		expect(relativeTime(new Date(now - 3 * 86_400_000).toISOString(), now)).toBe('3 days ago');
	});
});
