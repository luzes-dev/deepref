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

export function isShortcutSuppressed(target: EventTarget | null, overlayOpen = false): boolean {
	if (overlayOpen || !target || typeof target !== 'object') return overlayOpen;
	const element = target as ShortcutTarget;
	if (element.isContentEditable) return true;
	if (['INPUT', 'TEXTAREA', 'SELECT', 'BUTTON'].includes(element.tagName?.toUpperCase() ?? '')) {
		return true;
	}
	return Boolean(
		element.closest?.(
			'[role="dialog"], [role="menu"], [role="listbox"], [data-state="open"][data-slot="popover"], [data-state="open"][data-slot="dropdown-menu"], [data-state="open"][data-slot="select-content"]'
		)
	);
}
