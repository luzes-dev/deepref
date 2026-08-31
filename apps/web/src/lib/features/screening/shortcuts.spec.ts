import { describe, expect, it } from 'vitest';
import { hasOpenScreeningOverlay, isShortcutSuppressed, shortcutAction } from './shortcuts';

describe('screening shortcuts', () => {
	it('dispatches local decision, navigation, and undo keys', () => {
		expect(shortcutAction('i')).toBe('include');
		expect(shortcutAction('E')).toBe('exclude');
		expect(shortcutAction('m')).toBe('maybe');
		expect(shortcutAction('ArrowLeft')).toBe('previous');
		expect(shortcutAction('ArrowRight')).toBe('next');
		expect(shortcutAction('u')).toBe('undo');
		expect(shortcutAction('x')).toBeNull();
	});

	it('suppresses shortcuts in controls, editable content, and overlays', () => {
		expect(isShortcutSuppressed({ tagName: 'INPUT' } as unknown as EventTarget)).toBe(true);
		expect(isShortcutSuppressed({ tagName: 'button' } as unknown as EventTarget)).toBe(true);
		expect(isShortcutSuppressed({ isContentEditable: true } as unknown as EventTarget)).toBe(
			true
		);
		expect(isShortcutSuppressed(null, true)).toBe(true);
		expect(isShortcutSuppressed(null)).toBe(false);
	});

	it('detects an actually open overlay from its stable role or state', () => {
		const closest = (selector: string) =>
			selector.includes('[role="listbox"]') ? ({} as Element) : null;
		expect(isShortcutSuppressed({ closest } as unknown as EventTarget)).toBe(true);
		expect(
			isShortcutSuppressed({
				closest: (selector: string) =>
					selector.includes('[data-state="open"][data-slot="popover"]')
						? ({} as Element)
						: null
			} as unknown as EventTarget)
		).toBe(true);
	});

	it('checks the whole document so body-focused shortcuts respect open portals', () => {
		const open = { querySelector: () => ({}) } as unknown as Document;
		const closed = { querySelector: () => null } as unknown as Document;
		expect(hasOpenScreeningOverlay(open)).toBe(true);
		expect(hasOpenScreeningOverlay(closed)).toBe(false);
	});
});
