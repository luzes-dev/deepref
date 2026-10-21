import { describe, expect, it } from 'vitest';
import { ApiError } from '#lib/api/custom-fetch.js';
import type { MeResponse } from '#lib/api/generated/models/index.js';
import { sessionStatus } from './session-status';

const user = {
	id: 'u1',
	email: 'a@example.com',
	display_name: 'Ada'
} as unknown as NonNullable<MeResponse['user']>;

const answer = (me: MeResponse) => ({ isPending: false, data: { data: me } });

describe('sessionStatus', () => {
	it('is loading until /me answers', () => {
		expect(sessionStatus({ isPending: true })).toEqual({ kind: 'loading' });
		expect(sessionStatus({ isPending: false })).toEqual({ kind: 'loading' });
	});

	it('is disabled when the deployment does not require sign-in', () => {
		expect(sessionStatus(answer({ auth_enabled: false, user: null }))).toEqual({
			kind: 'disabled'
		});
	});

	it('is signed in when a user is returned', () => {
		expect(sessionStatus(answer({ auth_enabled: true, user }))).toEqual({
			kind: 'signed-in',
			user
		});
	});

	it('is signed out when auth is on and there is no user', () => {
		expect(sessionStatus(answer({ auth_enabled: true, user: null }))).toEqual({
			kind: 'signed-out'
		});
	});

	it('tells an expired session from a missing one', () => {
		const expired = new ApiError(401, 'expired', { code: 'SESSION_EXPIRED' });
		const missing = new ApiError(401, 'unauthenticated', { code: 'UNAUTHENTICATED' });
		expect(sessionStatus({ isPending: false, error: expired })).toEqual({ kind: 'expired' });
		expect(sessionStatus({ isPending: false, error: missing })).toEqual({ kind: 'signed-out' });
	});

	it('reports other failures as errors', () => {
		expect(sessionStatus({ isPending: false, error: new ApiError(500, 'boom', {}) })).toEqual({
			kind: 'error'
		});
		expect(sessionStatus({ isPending: false, error: new Error('network') })).toEqual({
			kind: 'error'
		});
	});
});
