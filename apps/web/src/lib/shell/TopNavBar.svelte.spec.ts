import { describe, it, expect } from 'vitest';
import { render, screen } from '@testing-library/svelte';
import TopNavBarTestHost from '#lib/tests/TopNavBarTestHost.svelte';

function renderTopNavBar(props: Record<string, unknown> = {}) {
	return render(TopNavBarTestHost, { props });
}

describe('TopNavBar component', () => {
	it('renders the default page title and notifications without a duplicate project switcher', () => {
		expect.hasAssertions();
		renderTopNavBar();

		expect(screen.getByRole('heading', { name: 'Overview' })).toBeInTheDocument();
		expect(screen.getByTestId('notifications-button')).toBeInTheDocument();
		expect(screen.queryByText('All Projects')).not.toBeInTheDocument();
		expect(screen.queryByText('Agent')).not.toBeInTheDocument();
	});

	it('renders a custom title', () => {
		expect.hasAssertions();
		renderTopNavBar({ title: 'Analytics' });

		expect(screen.getByRole('heading', { name: 'Analytics' })).toBeInTheDocument();
	});
});
