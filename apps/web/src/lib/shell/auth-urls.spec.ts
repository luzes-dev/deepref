import { describe, expect, it } from 'vitest';
import { loginUrl, returnPathFor, signInErrorMessage } from './auth-urls';

const at = (pathname: string, search = '') => ({ pathname, search });

describe('returnPathFor', () => {
	it('keeps a plain same-origin path and its query', () => {
		expect(returnPathFor(at('/projects/p1', '?tab=articles'))).toBe(
			'/projects/p1?tab=articles'
		);
	});

	it('falls back to the root for targets that are not plain paths', () => {
		expect(returnPathFor(at('//evil.example'))).toBe('/');
		expect(returnPathFor(at('projects'))).toBe('/');
		expect(returnPathFor(at('/a\\b'))).toBe('/');
		expect(returnPathFor(at('/a b'))).toBe('/');
		expect(returnPathFor(at('/a\u0001b'))).toBe('/');
	});

	it('never returns to the sign-in screen', () => {
		expect(returnPathFor(at('/signin'))).toBe('/');
		expect(returnPathFor(at('/signin', '?error=access_denied'))).toBe('/');
	});
});

describe('loginUrl', () => {
	it('encodes the return target', () => {
		expect(loginUrl('/a?b=1&c=2')).toBe('/api/auth/login?return_to=%2Fa%3Fb%3D1%26c%3D2');
	});
});

describe('signInErrorMessage', () => {
	it('maps known codes, generalises unknown ones, and ignores none', () => {
		expect(signInErrorMessage(null)).toBeNull();
		expect(signInErrorMessage('access_denied')).toBe('Sign-in was cancelled.');
		expect(signInErrorMessage('mystery')).toBe('Sign-in failed. Try again.');
	});
});
