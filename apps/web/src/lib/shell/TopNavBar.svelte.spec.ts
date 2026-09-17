import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/svelte';
import TopNavBarTestHost from '$lib/tests/TopNavBarTestHost.svelte';

function renderTopNavBar(props: Record<string, unknown> = {}) {
	return render(TopNavBarTestHost, { props });
}

describe('TopNavBar component', () => {
	it('renders default scope, title, and agent button', () => {
		expect.hasAssertions();
		renderTopNavBar();

		expect(screen.getByText('All Projects')).toBeInTheDocument();
		expect(screen.getByText('Overview')).toBeInTheDocument();
		expect(screen.getByText('Agent')).toBeInTheDocument();
	});

	it('renders custom title and scope label', () => {
		expect.hasAssertions();
		renderTopNavBar({
			title: 'Analytics',
			scopeLabel: 'My Workspace'
		});

		expect(screen.getByText('My Workspace')).toBeInTheDocument();
		expect(screen.getByText('Analytics')).toBeInTheDocument();
	});

	it('triggers onAgentClick when Agent button is clicked', async () => {
		expect.hasAssertions();
		const onAgentClick = vi.fn();
		renderTopNavBar({ onAgentClick });

		const agentButton = screen.getByRole('button', { name: /agent/i });
		expect(agentButton).toBeInTheDocument();
		await fireEvent.click(agentButton);
		expect(onAgentClick).toHaveBeenCalledTimes(1);
	});

	it('renders anchor tag when agentHref is provided', () => {
		expect.hasAssertions();
		renderTopNavBar({
			agentHref: '/projects/p1/assistant'
		});

		const agentLink = screen.getByRole('link', { name: /agent/i });
		expect(agentLink).toBeInTheDocument();
		expect(agentLink).toHaveAttribute('href', '/projects/p1/assistant');
	});

	it('derives selected project name when selectedProjectId matches', () => {
		expect.hasAssertions();
		const projects = [
			{ id: '1', name: 'Alpha Project' },
			{ id: '2', name: 'Beta Project' }
		];
		renderTopNavBar({
			projects,
			selectedProjectId: '2'
		});

		expect(screen.getByText('Beta Project')).toBeInTheDocument();
	});
});
