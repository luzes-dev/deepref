import { describe, it, expect } from 'vitest';
import { render, screen, fireEvent, waitFor } from '@testing-library/svelte';
import MswTestRunner from './MswTestRunner.svelte';
import { server } from './mocks/server';
import { networkOverrides } from './mocks/handlers';

describe('MSW component network simulation layer', () => {
	it('handles loading state with pending response', async () => {
		expect.hasAssertions();
		server.use(networkOverrides.loading('/api/projects', 'get', 5000));

		render(MswTestRunner, { mode: 'projects' });

		expect(screen.getByTestId('status-loading')).toBeInTheDocument();
		expect(screen.getByText('Loading projects...')).toBeInTheDocument();
	});

	it('handles success state with baseline mock data', async () => {
		expect.hasAssertions();
		render(MswTestRunner, { mode: 'projects' });

		await waitFor(() => {
			expect(screen.getByTestId('projects-list')).toBeInTheDocument();
		});
		expect(screen.getByText('Systematic Review of ML')).toBeInTheDocument();
	});

	it('handles empty state with override handler', async () => {
		expect.hasAssertions();
		server.use(networkOverrides.empty('/api/projects'));

		render(MswTestRunner, { mode: 'projects' });

		await waitFor(() => {
			expect(screen.getByTestId('status-empty')).toBeInTheDocument();
		});
		expect(screen.getByText('No projects found')).toBeInTheDocument();
	});

	it('handles validation error state on mutation', async () => {
		expect.hasAssertions();
		server.use(
			networkOverrides.validationError('/api/settings', {
				mailto: ['Must be a valid email address']
			})
		);

		render(MswTestRunner, { mode: 'settings' });

		await waitFor(() => {
			expect(screen.getByTestId('settings-input')).toBeInTheDocument();
		});

		const saveBtn = screen.getByTestId('save-btn');
		await fireEvent.click(saveBtn);

		await waitFor(() => {
			expect(screen.getByTestId('mutation-error')).toBeInTheDocument();
		});
		expect(
			screen.getByText(/Validation failed: One or more fields failed validation/)
		).toBeInTheDocument();
	});

	it('handles http failure state (500)', async () => {
		expect.hasAssertions();
		server.use(networkOverrides.httpFailure('/api/projects', 500, 'Database crashed'));

		render(MswTestRunner, { mode: 'projects' });

		await waitFor(() => {
			expect(screen.getByTestId('status-error')).toBeInTheDocument();
		});
		expect(screen.getByText(/Database crashed/)).toBeInTheDocument();
		expect(screen.getByText(/status: 500/)).toBeInTheDocument();
	});

	it('handles retryable failure state (503)', async () => {
		expect.hasAssertions();
		server.use(networkOverrides.retryableFailure('/api/projects'));

		render(MswTestRunner, { mode: 'projects' });

		await waitFor(() => {
			expect(screen.getByTestId('status-error')).toBeInTheDocument();
		});
		expect(
			screen.getByText(/Downstream provider rate limited or service busy/)
		).toBeInTheDocument();
		expect(screen.getByText(/status: 503/)).toBeInTheDocument();
	});

	it('handles degraded/partial response state (200 with degraded flag)', async () => {
		expect.hasAssertions();
		server.use(
			networkOverrides.degradedResponse('/api/projects', [
				{ id: 'deg-1', name: 'Cached Project Alpha' }
			])
		);

		render(MswTestRunner, { mode: 'projects' });

		await waitFor(() => {
			expect(screen.getByTestId('status-degraded')).toBeInTheDocument();
		});
		expect(screen.getByText('Operating in degraded mode')).toBeInTheDocument();
		expect(screen.getByText('Cached Project Alpha')).toBeInTheDocument();
	});
});
