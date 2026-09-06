import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import DependencyBanner from './DependencyBanner.svelte';
import type { DependencyStatus } from '$lib/api/generated/models';

describe('DependencyBanner component', () => {
	it('renders error alert when error is provided and triggers onRetry', async () => {
		expect.hasAssertions();
		const onRetry = vi.fn();
		render(DependencyBanner, {
			error: new Error('Failed to connect to backend'),
			onRetry
		});

		expect(screen.getByText('Dependency status unavailable')).toBeInTheDocument();
		expect(screen.getByText(/Failed to connect to backend/)).toBeInTheDocument();

		const refreshButton = screen.getByRole('button', { name: /refresh/i });
		expect(refreshButton).toBeInTheDocument();
		await fireEvent.click(refreshButton);
		expect(onRetry).toHaveBeenCalledTimes(1);
	});

	it('renders warning banner when dependencies are unavailable', () => {
		expect.hasAssertions();
		const onRetry = vi.fn();
		const status: DependencyStatus = {
			postgresql: {
				state: 'unavailable'
			},
			worker: {
				state: 'available'
			}
		};

		render(DependencyBanner, {
			status,
			onRetry
		});

		expect(screen.getByText('Core service interruption')).toBeInTheDocument();
		expect(screen.getByText(/postgresql: unavailable/)).toBeInTheDocument();
	});

	it('renders nothing when all dependencies are available and no error', () => {
		expect.hasAssertions();
		const onRetry = vi.fn();
		const status: DependencyStatus = {
			postgresql: {
				state: 'available'
			},
			worker: {
				state: 'available'
			}
		};

		const { container } = render(DependencyBanner, {
			status,
			onRetry
		});

		expect(container.textContent?.trim()).toBe('');
	});
});
