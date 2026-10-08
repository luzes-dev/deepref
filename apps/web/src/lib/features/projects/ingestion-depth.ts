import { IMPORT_FALLBACK_MAX_DEPTH } from './constants';

export type ImportDepthSource = 'remembered' | 'settings' | 'fallback';

export type ImportDepth = {
	depth: number;
	source: ImportDepthSource;
};

function isDepth(value: unknown): value is number {
	return typeof value === 'number' && Number.isSafeInteger(value) && value >= 0;
}

/**
 * The citation depth a DOI import starts from: the choice remembered for this project, then the
 * workspace Settings default, then the built-in fallback.
 */
export function resolveImportDepth(input: {
	remembered?: number;
	settingsDefault?: number;
}): ImportDepth {
	if (isDepth(input.remembered)) return { depth: input.remembered, source: 'remembered' };
	if (isDepth(input.settingsDefault)) return { depth: input.settingsDefault, source: 'settings' };
	return { depth: IMPORT_FALLBACK_MAX_DEPTH, source: 'fallback' };
}

/** Reads a depth saved in browser storage. Anything but a whole, non-negative number is ignored. */
export function parseRememberedDepth(raw: string | null): number | undefined {
	if (raw === null || raw.trim() === '') return undefined;
	const value = Number(raw);
	return isDepth(value) ? value : undefined;
}
