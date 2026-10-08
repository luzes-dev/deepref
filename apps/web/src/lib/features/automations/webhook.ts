/**
 * How to call a webhook automation: the signature a caller must send, and a
 * copyable command that does it.
 */

/** Stands in for the secret in the example so the copied text never holds it. */
export const SECRET_PLACEHOLDER = 'PASTE_THE_SECRET_HERE';

/** Plain-language rule for the signature header. */
export function signatureRule(header: string): string {
	return `Send the header ${header} with the value sha256= followed by the HMAC-SHA256 of the exact request body, keyed with the secret and written in lowercase hex. A call with a missing or wrong signature is refused.`;
}

/**
 * A POSIX shell (bash, zsh) command that posts a sample body to `url` with a
 * correct signature. The body is sent byte for byte as it was signed.
 */
export function webhookCurl(url: string, header: string, secret = SECRET_PLACEHOLDER): string {
	const body = '{"name":"example"}';
	return [
		`SECRET='${secret}'`,
		`BODY='${body}'`,
		`SIGNATURE=$(printf '%s' "$BODY" | openssl dgst -sha256 -hmac "$SECRET" | awk '{print $NF}')`,
		`curl -X POST '${url}' \\`,
		`  -H 'Content-Type: application/json' \\`,
		`  -H "${header}: sha256=$SIGNATURE" \\`,
		`  --data-binary "$BODY"`
	].join('\n');
}
