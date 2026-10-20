import { ApiError } from '#lib/api/custom-fetch.js';
import type { CurrentUserResponse, MeResponse } from '#lib/api/generated/models/index.js';

export type SessionStatus =
	| { kind: 'loading' }
	/** The deployment does not require sign-in. */
	| { kind: 'disabled' }
	| { kind: 'signed-in'; user: CurrentUserResponse }
	| { kind: 'signed-out' }
	| { kind: 'expired' }
	| { kind: 'error' };

export type SessionQuery = {
	isPending: boolean;
	data?: { data: MeResponse } | undefined;
	error?: unknown;
};

/** Reads the `/me` query as the one question the shell asks: who is using this tab? */
export function sessionStatus(query: SessionQuery): SessionStatus {
	if (query.error) {
		if (query.error instanceof ApiError && query.error.status === 401) {
			return query.error.code === 'SESSION_EXPIRED'
				? { kind: 'expired' }
				: { kind: 'signed-out' };
		}
		return { kind: 'error' };
	}
	if (query.isPending || !query.data) return { kind: 'loading' };
	const me = query.data.data;
	if (!me.auth_enabled) return { kind: 'disabled' };
	return me.user ? { kind: 'signed-in', user: me.user } : { kind: 'signed-out' };
}
