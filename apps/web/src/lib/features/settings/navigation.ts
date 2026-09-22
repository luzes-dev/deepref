import { goto, preloadData, pushState } from '$app/navigation';
import { page } from '$app/state';
import { resolve } from '$app/paths';

const settingsHref = resolve('/settings');
let settingsTrigger: HTMLElement | null = null;
type SettingsTransition = 'expand' | 'collapse';
let pendingSettingsTransition: SettingsTransition | null = null;

function isModifiedClick(event: MouseEvent): boolean {
	return event.button !== 0 || event.metaKey || event.ctrlKey || event.shiftKey || event.altKey;
}

function pageLocation(url: URL): string {
	return `${url.pathname}${url.search}${url.hash}`;
}

function navigateNormally(): void {
	// fallow-ignore-next-line security-sink -- Fallback target is the constant settings route path, never user input
	void goto(settingsHref).catch(() => window.location.assign(settingsHref));
}

function requestSettingsTransition(transition: SettingsTransition): void {
	pendingSettingsTransition = transition;
}

/**
 * Consume the transition requested by the next Settings navigation.
 *
 * The request lives outside page state on purpose: it only coordinates the
 * current DOM update with the native View Transition API. The history entry
 * remains the source of truth for restoring the overlay and its origin.
 */
export function consumeSettingsTransition(): 'expand' | 'collapse' | null {
	const transition = pendingSettingsTransition;
	pendingSettingsTransition = null;
	return transition;
}

/**
 * Progressively enhance a real Settings link into a shallow overlay.
 *
 * Modified clicks intentionally fall through to the browser so open-in-new-tab,
 * copied-link, and JavaScript-disabled behavior continue to work as normal.
 */
export async function openSettingsFromLink(event: MouseEvent): Promise<void> {
	if (event.defaultPrevented || isModifiedClick(event)) return;

	const target = event.currentTarget;
	if (!(target instanceof HTMLAnchorElement)) return;

	const href = target.href;
	settingsTrigger = target;
	const backgroundUrl = pageLocation(page.url);
	const requestId = ++openSettingsRequestId;
	event.preventDefault();

	try {
		const result = await preloadData(href);
		if (requestId !== openSettingsRequestId) return;

		// A route change while preload was in flight means this click no longer
		// belongs to the captured background page. Let that navigation stand.
		if (pageLocation(page.url) !== backgroundUrl || page.state.settingsOverlay) return;

		if (result.type === 'loaded' && result.status === 200) {
			pushState(settingsHref, {
				...page.state,
				settingsOverlay: { backgroundUrl }
			});
			return;
		}

		navigateNormally();
	} catch {
		if (
			requestId === openSettingsRequestId &&
			pageLocation(page.url) === backgroundUrl &&
			!page.state.settingsOverlay
		) {
			navigateNormally();
		}
	}
}

let openSettingsRequestId = 0;

export function closeSettingsOverlay(): void {
	if (page.state.settingsOverlay) history.back();
}

export function expandSettings(): void {
	const overlay = page.state.settingsOverlay;
	if (!overlay) return;

	requestSettingsTransition('expand');

	// Keep every other state field (for example transient search state), while
	// removing the overlay marker from the real full-page entry. The overlay
	// history entry itself is left untouched so history.back() returns to the
	// exact origin URL and state.
	const nextState = { ...page.state };
	delete nextState.settingsOverlay;
	delete nextState.settingsExpansion;
	nextState.settingsExpansion = { backgroundUrl: overlay.backgroundUrl };

	// fallow-ignore-next-line security-sink -- Fallback target is the constant settings route path, never user input
	void goto(settingsHref, { state: nextState }).catch(() => window.location.assign(settingsHref));
}

export function collapseSettings(): void {
	if (!page.state.settingsExpansion) return;

	requestSettingsTransition('collapse');
	history.back();
}

export function getSettingsTrigger(): HTMLElement | null {
	return settingsTrigger;
}
