export const CROSSREF_MAILTO_REQUIRED = 'Crossref mailto is required.';
export const CROSSREF_MAILTO_INVALID = 'Enter an e-mail address such as research@example.org.';

/**
 * Shape check for the Crossref contact address. It mirrors `is_plausible_email`
 * in crates/http-api/src/routes/settings.rs, so the browser and the server agree
 * on what is malformed.
 */
const CONTACT_EMAIL_PATTERN = /^[^\s@]+@(?:[^\s@.]+\.)+[^\s@.]{2,}$/;
const MAX_EMAIL_LENGTH = 254;

/** Returns the inline error for a Crossref contact value, or undefined when it is usable. */
export function crossrefMailtoError(value: string): string | undefined {
	const trimmed = value.trim();
	if (!trimmed) return CROSSREF_MAILTO_REQUIRED;
	if (trimmed.length > MAX_EMAIL_LENGTH || !CONTACT_EMAIL_PATTERN.test(trimmed)) {
		return CROSSREF_MAILTO_INVALID;
	}
	return undefined;
}
