import { afterEach, describe, expect, it, vi } from 'vitest';
import { saveReviewerId } from '#lib/api/reviewer';
import { confirmAssistantPlan, createAssistantConversation } from './chat-api';

afterEach(() => vi.unstubAllGlobals());

describe('assistant reviewer attribution', () => {
	it('uses the current reviewer at each request and forwards record deselections', async () => {
		const entries = new Map<string, string>();
		vi.stubGlobal('window', {
			sessionStorage: {
				getItem: (key: string) => entries.get(key) ?? null,
				setItem: (key: string, value: string) => entries.set(key, value)
			}
		});
		const fetch = vi.fn().mockImplementation(
			async () =>
				new Response('{}', {
					status: 200,
					headers: { 'Content-Type': 'application/json' }
				})
		);
		vi.stubGlobal('fetch', fetch);
		saveReviewerId('reviewer-a');
		await createAssistantConversation('project', 'Review');
		const first: RequestInit = fetch.mock.calls[0][1];
		expect(new Headers(first.headers).get('x-actor-id')).toBe('reviewer-a');
		saveReviewerId('reviewer-b');
		const deselected = [{ action_id: 'a1', report_id: 'report' }];
		await confirmAssistantPlan('project', 'plan', deselected);
		const second: RequestInit = fetch.mock.calls[1][1];
		expect(new Headers(second.headers).get('x-actor-id')).toBe('reviewer-b');
		expect(second.body).toBe(JSON.stringify({ deselected }));
	});
});
