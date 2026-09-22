// @vitest-environment jsdom
import { beforeEach, describe, expect, it, vi } from 'vitest';

const navigation = vi.hoisted(() => ({
	preloadData: vi.fn<() => Promise<{ type: string; status: number }>>(),
	pushState: vi.fn(),
	goto: vi.fn<() => Promise<void>>()
}));
const current = vi.hoisted(() => {
	const state: App.PageState = {};
	return {
		page: {
			url: new URL('http://localhost/projects/example/articles?filter=review'),
			state
		}
	};
});
vi.mock('$app/navigation', () => navigation);
vi.mock('$app/state', () => current);
vi.mock('$app/paths', () => ({ resolve: (path: string) => path }));

import { expandSettings, openSettingsFromLink } from './navigation';

function clickSettings(options: MouseEventInit = {}) {
	const link = document.createElement('a');
	link.href = 'http://localhost/settings';
	const event = new MouseEvent('click', { cancelable: true, ...options });
	Object.defineProperty(event, 'currentTarget', { value: link });
	return { event, done: openSettingsFromLink(event) };
}

beforeEach(() => {
	vi.resetAllMocks();
	current.page.url = new URL('http://localhost/projects/example/articles?filter=review');
	current.page.state = {};
	navigation.goto.mockResolvedValue();
	navigation.preloadData.mockResolvedValue({ type: 'loaded', status: 200 });
});

describe('Settings link enhancement', () => {
	it.each([{ ctrlKey: true }, { metaKey: true }, { shiftKey: true }, { button: 1 }])(
		'preserves native link behavior for %j',
		async (options) => {
			const { event, done } = clickSettings(options);
			await done;
			expect(event.defaultPrevented).toBe(false);
			expect(navigation.preloadData).not.toHaveBeenCalled();
			expect(navigation.pushState).not.toHaveBeenCalled();
		}
	);

	it('falls back to real navigation when preload returns an error', async () => {
		navigation.preloadData.mockResolvedValue({ type: 'loaded', status: 500 });
		await clickSettings().done;
		expect(navigation.goto).toHaveBeenCalledWith('/settings');
		expect(navigation.pushState).not.toHaveBeenCalled();
	});

	it('falls back to real navigation when preload throws', async () => {
		navigation.preloadData.mockRejectedValue(new Error('Chunk unavailable'));
		await clickSettings().done;
		expect(navigation.goto).toHaveBeenCalledWith('/settings');
		expect(navigation.pushState).not.toHaveBeenCalled();
	});

	it('does not reopen Settings after the user leaves while preloading', async () => {
		const preload = Promise.withResolvers<{ type: string; status: number }>();
		navigation.preloadData.mockReturnValue(preload.promise);
		const { done } = clickSettings();
		current.page.url = new URL('http://localhost/projects/example/overview');
		preload.resolve({ type: 'loaded', status: 200 });
		await done;
		expect(navigation.pushState).not.toHaveBeenCalled();
		expect(navigation.goto).not.toHaveBeenCalled();
	});

	it('creates only one history entry for overlapping clicks and preserves page state', async () => {
		current.page.state = { deeprefFullTextSearch: '?queue=missing' };
		const preload = Promise.withResolvers<{ type: string; status: number }>();
		navigation.preloadData.mockReturnValue(preload.promise);
		const first = clickSettings();
		const second = clickSettings();
		preload.resolve({ type: 'loaded', status: 200 });
		await Promise.all([first.done, second.done]);
		expect(navigation.pushState).toHaveBeenCalledTimes(1);
		expect(navigation.pushState).toHaveBeenCalledWith(
			'/settings',
			expect.objectContaining({ deeprefFullTextSearch: '?queue=missing' })
		);
	});

	it('expands the overlay into the standalone Settings route without carrying overlay state', () => {
		current.page.state = {
			settingsOverlay: { backgroundUrl: '/projects/example/articles?filter=review' },
			deeprefFullTextSearch: '?queue=missing'
		};

		expandSettings();

		expect(navigation.goto).toHaveBeenCalledWith('/settings', {
			state: {
				deeprefFullTextSearch: '?queue=missing',
				settingsExpansion: {
					backgroundUrl: '/projects/example/articles?filter=review'
				}
			}
		});
	});
});
