export const SIGN_IN_PATH = '/signin';

function hasUnsafeCharacter(path: string): boolean {
	for (const char of path) {
		if (char === '\\' || char.trim() === '' || char.charCodeAt(0) < 0x20) return true;
	}
	return false;
}

/**
 * The path and query to return to after sign-in, or `/` when the location is
 * not a plain same-origin path. The server validates again; this keeps an
 * obviously unusable target out of the request.
 */
export function returnPathFor(location: { pathname: string; search: string }): string {
	const path = `${location.pathname}${location.search}`;
	if (!path.startsWith('/') || path.startsWith('//') || hasUnsafeCharacter(path)) return '/';
	if (path === SIGN_IN_PATH || path.startsWith(`${SIGN_IN_PATH}?`)) return '/';
	return path;
}

/** The API route that starts sign-in. It is a top-level navigation, not a fetch. */
export function loginUrl(returnTo: string): string {
	return `/api/auth/login?return_to=${encodeURIComponent(returnTo)}`;
}

const SIGN_IN_ERRORS: Record<string, string> = {
	invalid_state: 'That sign-in attempt expired or did not start in this browser. Try again.',
	access_denied: 'Sign-in was cancelled.',
	invalid_code: 'The sign-in link was no longer valid. Try again.',
	provider_unavailable: 'The sign-in service is unavailable right now. Try again in a moment.',
	provisioning_failed: 'Your account could not be set up. Try again, or contact support.',
	account_deactivated: 'This account has been deactivated. Contact support if this is a mistake.'
};

export function signInErrorMessage(code: string | null): string | null {
	if (!code) return null;
	return SIGN_IN_ERRORS[code] ?? 'Sign-in failed. Try again.';
}
