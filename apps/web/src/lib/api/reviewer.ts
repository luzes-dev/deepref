const REVIEWER_STORAGE_KEY = 'deepref-reviewer-id';
const DEFAULT_REVIEWER_ID = 'local-user';

/** Local actor attribution only; this does not authenticate a person. */
export function currentReviewerId(): string {
	if (typeof window === 'undefined') return DEFAULT_REVIEWER_ID;
	try {
		return window.sessionStorage.getItem(REVIEWER_STORAGE_KEY) ?? DEFAULT_REVIEWER_ID;
	} catch {
		return DEFAULT_REVIEWER_ID;
	}
}

export function saveReviewerId(value: string): void {
	const id = value.trim();
	if (!/^[\x21-\x7e]{1,128}$/.test(id)) {
		throw new TypeError('Use 1–128 printable ASCII characters without spaces.');
	}
	if (typeof window === 'undefined') throw new Error('Reviewer identity requires a browser.');
	window.sessionStorage.setItem(REVIEWER_STORAGE_KEY, id);
}
