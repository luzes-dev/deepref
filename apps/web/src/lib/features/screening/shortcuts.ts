export type ScreeningShortcutAction =
	'include' | 'exclude' | 'maybe' | 'previous' | 'next' | 'undo';

export function shortcutAction(key: string): ScreeningShortcutAction | null {
	switch (key.toLowerCase()) {
		case 'i':
			return 'include';
		case 'e':
			return 'exclude';
		case 'm':
			return 'maybe';
		case 'u':
			return 'undo';
		case 'arrowleft':
			return 'previous';
		case 'arrowright':
			return 'next';
		default:
			return null;
	}
}

type ShortcutTarget = EventTarget & {
	tagName?: string;
	isContentEditable?: boolean;
	closest?: (selector: string) => Element | null;
};

const openOverlaySelector = [
	'[role="dialog"]:not([hidden]):not([aria-hidden="true"]):not([data-state="closed"])',
	'[role="menu"]:not([hidden]):not([aria-hidden="true"]):not([data-state="closed"])',
	'[role="listbox"]:not([hidden]):not([aria-hidden="true"]):not([data-state="closed"])',
	'[data-state="open"][data-slot="popover"]',
	'[data-state="open"][data-slot="popover-content"]',
	'[data-state="open"][data-slot="dropdown-menu"]',
	'[data-state="open"][data-slot="select-content"]'
].join(',');

export function hasOpenScreeningOverlay(ownerDocument?: Document): boolean {
	const document =
		ownerDocument ??
		(typeof globalThis.document === 'undefined' ? undefined : globalThis.document);
	return Boolean(document?.querySelector(openOverlaySelector));
}

const textEntryTags = ['INPUT', 'TEXTAREA', 'SELECT'];

const insideOverlaySelector =
	'[role="dialog"], [role="menu"], [role="listbox"], [data-state="open"][data-slot="popover"], [data-state="open"][data-slot="dropdown-menu"], [data-state="open"][data-slot="select-content"]';

/**
 * Buttons deliberately do not suppress the decision keys. A reviewer who clicks
 * "Include" or a queue row must still be able to press M or E next; the browser
 * already gives Enter and Space to the focused button. Keys are held back only
 * while typing into a field and inside an open menu, listbox or dialog.
 */
export function isShortcutSuppressed(target: EventTarget | null, overlayOpen = false): boolean {
	if (overlayOpen || !target || typeof target !== 'object') return overlayOpen;
	const element = target as ShortcutTarget;
	if (element.isContentEditable) return true;
	if (textEntryTags.includes(element.tagName?.toUpperCase() ?? '')) return true;
	return Boolean(element.closest?.(insideOverlaySelector));
}

/** Browser and OS shortcuts (Ctrl+E, Cmd+M, ...) must never record a screening decision. */
export function hasCommandModifier(
	event: Partial<Pick<KeyboardEvent, 'altKey' | 'ctrlKey' | 'metaKey' | 'shiftKey'>>
): boolean {
	return Boolean(event.altKey || event.ctrlKey || event.metaKey);
}
