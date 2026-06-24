import { describe, expect, it } from 'vitest';
import { ApiError } from './custom-fetch';
import { shouldRetryQuery } from './query-client';

describe('shouldRetryQuery', () => {
	it('retries a network failure once', () => {
		expect(shouldRetryQuery(0, new Error('offline'))).toBe(true);
		expect(shouldRetryQuery(1, new Error('offline'))).toBe(false);
	});

	it('does not retry client errors', () => {
		expect(shouldRetryQuery(0, new ApiError(400, 'invalid', { error: 'invalid' }))).toBe(false);
	});

	it('retries server errors once', () => {
		const error = new ApiError(503, 'unavailable', { error: 'unavailable' });
		expect(shouldRetryQuery(0, error)).toBe(true);
		expect(shouldRetryQuery(1, error)).toBe(false);
	});
});
