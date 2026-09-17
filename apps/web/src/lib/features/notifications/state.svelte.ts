import type { NotificationDto } from '$lib/api/generated/models';

const STORAGE_KEY = 'deepref:notifications:last-seen-revision';
const ARRIVAL_WINDOW_MS = 10 * 60 * 1000;

let lastSeenRevision: number | null = loadPersisted();
let panelOpen = false;

function loadPersisted(): number | null {
	if (typeof localStorage === 'undefined') return null;
	const raw = localStorage.getItem(STORAGE_KEY);
	if (raw === null) return null;
	const parsed = Number(raw);
	return Number.isFinite(parsed) ? parsed : null;
}

function persist(revision: number): void {
	lastSeenRevision = revision;
	if (typeof localStorage === 'undefined') return;
	localStorage.setItem(STORAGE_KEY, String(revision));
}

export function setNotificationPanelOpen(open: boolean): void {
	panelOpen = open;
}

export function isNotificationPanelOpen(): boolean {
	return panelOpen;
}

/**
 * Classify freshly polled list rows. The first observation ever (no persisted
 * revision) silences history; afterwards rows newer than the persisted
 * revision that are still unread and recent enough are returned as arrivals.
 */
export function observeNotifications(items: NotificationDto[]): NotificationDto[] {
	const latest = items.reduce((max, item) => Math.max(max, item.revision), lastSeenRevision ?? 0);
	if (lastSeenRevision === null) {
		persist(latest);
		return [];
	}
	const arrivals = items.filter(
		(item) =>
			item.revision > lastSeenRevision! &&
			item.read_at === null &&
			Date.parse(item.created_at) >= Date.now() - ARRIVAL_WINDOW_MS
	);
	persist(latest);
	return arrivals;
}

export function relativeTime(iso: string, now = Date.now()): string {
	const formatter = new Intl.RelativeTimeFormat('en', { numeric: 'auto' });
	const seconds = Math.round((Date.parse(iso) - now) / 1000);
	const table: Array<{ limit: number; unit: Intl.RelativeTimeFormatUnit; divisor: number }> = [
		{ limit: 60, unit: 'second', divisor: 1 },
		{ limit: 3600, unit: 'minute', divisor: 60 },
		{ limit: 86400, unit: 'hour', divisor: 3600 },
		{ limit: 604800, unit: 'day', divisor: 86400 },
		{ limit: Number.POSITIVE_INFINITY, unit: 'week', divisor: 604800 }
	];
	for (const entry of table) {
		if (Math.abs(seconds) < entry.limit) {
			return formatter.format(Math.round(seconds / entry.divisor), entry.unit);
		}
	}
	return formatter.format(0, 'second');
}
