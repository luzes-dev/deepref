import { afterEach, describe, expect, it, vi } from 'vitest';
import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/svelte';
import { QueryClient } from '@tanstack/svelte-query';
import Harness from '../../../tests/AccountSettingsHarness.svelte';

vi.mock('$app/state', () => ({
	page: { params: {}, state: {}, url: new URL('http://localhost/settings') }
}));

const issued = 'dr_key_v1_ISSUED_SECRET';
const rotated = 'dr_key_v1_ROTATED_SECRET';
const key = {
	id: 'key-a',
	workspace_id: 'workspace-a',
	created_by_user_id: 'user',
	name: 'Reader',
	prefix: 'dr_key_v1_',
	scopes: ['project:view'],
	created_at: '2026-10-27T00:00:00Z'
};
function json(value: unknown, status = 200) {
	return new Response(JSON.stringify(value), {
		status,
		headers: { 'content-type': 'application/json' }
	});
}
function setup(
	view: 'keys' | 'billing' | 'privacy' | 'account' | 'settings',
	fetcher: (request: Request) => Promise<Response> | Response
) {
	vi.stubGlobal('fetch', vi.fn(fetcher));
	const client = new QueryClient({
		defaultOptions: { queries: { retry: false }, mutations: { retry: false } }
	});
	const navigate = vi.fn<(url: string) => void>();
	const result = render(Harness, { client, view, workspaceId: 'workspace-a', navigate });
	return { client, navigate, ...result };
}
afterEach(() => {
	cleanup();
	vi.unstubAllGlobals();
});

describe('account settings', () => {
	it('shows created and rotated keys once, keeps secrets out of query/mutation caches, and confirms revocation', async () => {
		let created = false;
		let revoked = false;
		const requests: Request[] = [];
		const { client, rerender } = setup('keys', (request) => {
			requests.push(request);
			if (request.method === 'GET')
				return json(created ? [{ ...key, revoked_at: revoked ? '2026-10-27' : null }] : []);
			if (request.method === 'DELETE') {
				revoked = true;
				return new Response(null, { status: 204 });
			}
			created = true;
			return json({ key, secret: request.url.endsWith('/rotate') ? rotated : issued }, 201);
		});
		await screen.findByText('No API keys in this workspace.');
		await fireEvent.input(screen.getByLabelText('Key name'), { target: { value: 'Reader' } });
		await fireEvent.click(screen.getByRole('button', { name: 'Create API key' }));
		expect(await screen.findByTestId('issued-api-key')).toHaveTextContent(issued);
		expect(
			JSON.stringify(
				client
					.getMutationCache()
					.getAll()
					.map((entry) => entry.state)
			)
		).not.toContain(issued);
		expect(
			JSON.stringify(
				client
					.getQueryCache()
					.getAll()
					.map((entry) => entry.state)
			)
		).not.toContain(issued);
		await fireEvent.click(screen.getByRole('button', { name: 'I saved the key' }));
		expect(screen.queryByText(issued)).not.toBeInTheDocument();
		await fireEvent.click(await screen.findByRole('button', { name: 'Rotate Reader' }));
		expect(requests.filter((request) => request.url.endsWith('/rotate'))).toHaveLength(0);
		await fireEvent.click(screen.getByRole('button', { name: 'Confirm rotate' }));
		expect(await screen.findByTestId('issued-api-key')).toHaveTextContent(rotated);
		expect(
			JSON.stringify(
				client
					.getMutationCache()
					.getAll()
					.map((entry) => entry.state)
			)
		).not.toContain(rotated);
		await rerender({ client, view: 'keys', workspaceId: 'workspace-b' });
		expect(screen.queryByText(rotated)).not.toBeInTheDocument();
		await fireEvent.click(await screen.findByRole('button', { name: 'Revoke Reader' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Confirm revoke' }));
		await screen.findByText(/Revoked/);
	});

	it('reports key failure and prevents duplicate requests while pending', async () => {
		const pending = Promise.withResolvers<Response>();
		let posts = 0;
		setup('keys', (request) => {
			if (request.method === 'GET') return json([]);
			posts++;
			return pending.promise;
		});
		await screen.findByText('No API keys in this workspace.');
		await fireEvent.input(screen.getByLabelText('Key name'), { target: { value: 'Reader' } });
		await fireEvent.click(screen.getByRole('button', { name: 'Create API key' }));
		await waitFor(() =>
			expect(screen.getByRole('button', { name: 'Create API key' })).toBeDisabled()
		);
		await fireEvent.click(screen.getByRole('button', { name: 'Create API key' }));
		expect(posts).toBe(1);
		pending.resolve(json({ error: 'resource not found' }, 404));
		await screen.findByText(/The key change failed/);
		expect(screen.queryByTestId('issued-api-key')).not.toBeInTheDocument();
	});

	it('opens hosted checkout with explicit contact/closed offering and never caches the link', async () => {
		const requests: Array<{ path: string; body: unknown }> = [];
		const link = 'https://test.checkout.dodopayments.com/session/BEARER';
		const { client, navigate } = setup('billing', async (request) => {
			requests.push({ path: new URL(request.url).pathname, body: await request.json() });
			return request.url.endsWith('/customer')
				? new Response(null, { status: 204 })
				: json({ url: link });
		});
		await fireEvent.input(screen.getByLabelText('Billing name'), {
			target: { value: 'Research owner' }
		});
		await fireEvent.input(screen.getByLabelText('Billing email'), {
			target: { value: 'billing@example.org' }
		});
		await fireEvent.click(screen.getByRole('button', { name: 'Continue to checkout' }));
		await waitFor(() => expect(navigate).toHaveBeenCalledWith(link));
		expect(requests[0]?.body).toEqual({ name: 'Research owner', email: 'billing@example.org' });
		expect(requests[1]?.body).toEqual({ offering: 'primary', return_path: '/settings' });
		expect(
			JSON.stringify(
				client
					.getMutationCache()
					.getAll()
					.map((entry) => entry.state)
			)
		).not.toContain('BEARER');
		expect(screen.getByText(/Returning here does not confirm a payment/)).toBeInTheDocument();
	});

	it('reports portal failure and refuses unsafe hosted links', async () => {
		const { navigate } = setup('billing', () => json({ url: 'https://evil.example/steal' }));
		await fireEvent.click(screen.getByRole('button', { name: 'Open customer portal' }));
		await screen.findByRole('alert');
		expect(navigate).not.toHaveBeenCalled();
	});

	it('requests, refreshes and cancels privacy export with visible coverage', async () => {
		let record: { id: string; status: string; requested_at: string } | null = null;
		setup('privacy', (request) => {
			if (request.method === 'POST') {
				record = { id: 'export-a', status: 'requested', requested_at: '2026-10-27' };
				return json(record, 202);
			}
			if (request.method === 'DELETE') {
				if (record) record.status = 'cancelled';
				return new Response(null, { status: 204 });
			}
			return json(record);
		});
		await screen.findByText('You have not requested an export.');
		await fireEvent.click(screen.getByText('Coverage and limits'));
		expect(screen.getByText(/covered rows are never silently dropped/)).toBeInTheDocument();
		await fireEvent.click(screen.getByRole('button', { name: 'Request data export' }));
		await screen.findByText('Export requested');
		expect(screen.getByRole('button', { name: 'Request data export' })).toBeDisabled();
		await fireEvent.click(screen.getByRole('button', { name: 'Cancel export' }));
		await screen.findByText('Export cancelled');
	});

	it('provides a mediated download only for an unexpired completed artifact', async () => {
		setup('privacy', () =>
			json({
				id: 'export-a',
				status: 'completed',
				requested_at: '2026-10-27',
				retained_until: '2099-10-27T00:00:00Z',
				size_bytes: 42,
				sha256: 'verified-hash'
			})
		);
		const link = await screen.findByRole('link', { name: 'Download ZIP' });
		expect(link).toHaveAttribute('href', '/api/me/export/export-a/download');
		expect(screen.getByText('SHA-256: verified-hash')).toBeInTheDocument();
	});
	it('shows the actual historical manifest and never offers an expired archive download', async () => {
		setup('privacy', () =>
			json({
				id: 'old-export',
				status: 'completed',
				requested_at: '2020-01-01',
				retained_until: '2020-01-08T00:00:00Z',
				scope_manifest: {
					version: 1,
					json_sections: ['profile.json', 'records.json'],
					exclusions: ['derived scientific output excluded in version 1']
				}
			})
		);
		await screen.findByText('Export completed');
		await fireEvent.click(screen.getByText('Exact coverage of this archive'));
		expect(screen.getByText('Sections: profile.json, records.json')).toBeInTheDocument();
		expect(
			screen.getByText('Exclusions: derived scientific output excluded in version 1')
		).toBeInTheDocument();
		expect(screen.queryByRole('link', { name: 'Download ZIP' })).not.toBeInTheDocument();
	});
	it('paginates workspace discovery and keeps privacy usable when global settings are forbidden', async () => {
		const paths: string[] = [];
		setup('settings', (request) => {
			const url = new URL(request.url);
			paths.push(url.pathname + url.search);
			if (url.pathname === '/api/settings') return json({ error: 'forbidden' }, 403);
			if (url.pathname === '/api/me/workspaces')
				return json(
					url.search
						? {
								items: [
									{
										id: 'workspace-b',
										name: 'Second workspace',
										can_manage_keys: false,
										can_manage_billing: false
									}
								],
								next_cursor: null
							}
						: {
								items: [
									{
										id: 'workspace-a',
										name: 'First workspace',
										can_manage_keys: false,
										can_manage_billing: false
									}
								],
								next_cursor: 'workspace-a'
							}
				);
			return json(null);
		});
		await fireEvent.click(screen.getByRole('button', { name: 'Account' }));
		await screen.findByText('You have not requested an export.');
		await fireEvent.click(await screen.findByRole('button', { name: 'Load more workspaces' }));
		await waitFor(() => expect(paths).toContain('/api/me/workspaces?after=workspace-a'));
		await fireEvent.click(screen.getByRole('combobox', { name: 'Workspace' }));
		await fireEvent.pointerUp(await screen.findByRole('option', { name: 'Second workspace' }), {
			pointerType: 'mouse'
		});
		await screen.findByText('Your current workspace role cannot manage API keys.');
		expect(
			screen.getByText('Your current workspace role cannot manage billing.')
		).toBeInTheDocument();
	});
	it('clears the once-only secret after an authorization refresh fails', async () => {
		let denied = false;
		setup('keys', (request) => {
			if (request.method !== 'GET') return json({ key, secret: issued }, 201);
			return denied ? json({ error: 'not found' }, 404) : json([]);
		});
		await screen.findByText('No API keys in this workspace.');
		await fireEvent.input(screen.getByLabelText('Key name'), { target: { value: 'Reader' } });
		await fireEvent.click(screen.getByRole('button', { name: 'Create API key' }));
		await screen.findByTestId('issued-api-key');
		denied = true;
		await fireEvent.click(screen.getByRole('button', { name: 'Refresh keys' }));
		await screen.findByRole('alert');
		expect(screen.queryByTestId('issued-api-key')).not.toBeInTheDocument();
		denied = false;
		await fireEvent.click(screen.getByRole('button', { name: 'Refresh keys' }));
		await screen.findByText('No API keys in this workspace.');
		expect(screen.queryByTestId('issued-api-key')).not.toBeInTheDocument();
	});
});
