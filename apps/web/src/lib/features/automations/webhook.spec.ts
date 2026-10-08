import { describe, expect, it } from 'vitest';
import { SECRET_PLACEHOLDER, signatureRule, webhookCurl } from './webhook';

describe('signatureRule', () => {
	it('names the header and the exact signature format', () => {
		const text = signatureRule('X-DeepRef-Signature');
		expect(text).toContain('X-DeepRef-Signature');
		expect(text).toContain('sha256=');
		expect(text).toContain('HMAC-SHA256');
		expect(text).toContain('exact request body');
		expect(text).toContain('lowercase hex');
	});
});

describe('webhookCurl', () => {
	const url = 'http://127.0.0.1:5197/api/hooks/workflows/abc123';

	it('posts to the real address with the signature header and the same body that was signed', () => {
		const script = webhookCurl(url, 'X-DeepRef-Signature');
		expect(script).toContain(`curl -X POST '${url}'`);
		expect(script).toContain('-H "X-DeepRef-Signature: sha256=$SIGNATURE"');
		expect(script).toContain('openssl dgst -sha256 -hmac "$SECRET"');
		expect(script).toContain('--data-binary "$BODY"');
		expect(script).toContain('printf \'%s\' "$BODY"');
	});

	it('does not put the real secret into the copied text by default', () => {
		const script = webhookCurl(url, 'X-DeepRef-Signature');
		expect(script).toContain(`SECRET='${SECRET_PLACEHOLDER}'`);
	});

	it('uses the secret it is given when asked', () => {
		const script = webhookCurl(url, 'X-DeepRef-Signature', 'a-secret');
		expect(script).toContain("SECRET='a-secret'");
		expect(script).not.toContain(SECRET_PLACEHOLDER);
	});
});
