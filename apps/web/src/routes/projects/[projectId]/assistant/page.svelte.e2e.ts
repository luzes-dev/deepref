import { expect, test, type Page } from '@playwright/test';

const api = 'http://localhost:4173/api';
const project = {
	id: 'project-1',
	name: 'Assistant project',
	description: 'A mocked project',
	default_max_depth: 2,
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-01T00:00:00Z'
};
const dependencies = {
	postgresql: { state: 'available', lag: null, backlog: null, oldest_age_seconds: null },
	worker: { state: 'available', lag: 0, backlog: 0, oldest_age_seconds: null }
};

const conversationId = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa1';
const secondConversationId = 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaa2';
const messageId = 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb1';
const reviewRunId = 'cccccccc-cccc-4ccc-8ccc-ccccccccccc1';

function conversation(id: string, title: string, updatedAt: string) {
	return {
		id,
		project_id: 'project-1',
		title,
		created_at: '2026-01-01T00:00:00Z',
		updated_at: updatedAt
	};
}

function assistantMessage(id: string, createdAt: string) {
	return {
		id,
		conversation_id: conversationId,
		role: 'assistant',
		content: 'The protocol targets adults with type 2 diabetes.',
		tool_calls: [
			{
				id: 'call-1',
				tool: 'get_project_protocol',
				args: { project_id: 'project-1' }
			}
		],
		tool_results: [
			{
				tool_call_id: 'call-1',
				tool: 'get_project_protocol',
				output: { id: 'protocol-1', name: 'Protocol v1' },
				proposal_review_run_id: null
			},
			{
				tool_call_id: 'call-2',
				tool: 'propose_screening_decision',
				output: {},
				proposal_review_run_id: reviewRunId
			}
		],
		metadata: null,
		created_at: createdAt
	};
}

function sseBody(): string {
	return [
		'event: token',
		'data: {"delta":"Reading the protocol"}',
		'',
		'event: tool_start',
		'data: {"tool":"get_project_protocol","tool_call_id":"call-1","args":{"project_id":"project-1"}}',
		'',
		'event: tool_complete',
		'data: {"tool":"get_project_protocol","tool_call_id":"call-1","output":{"id":"protocol-1","name":"Protocol v1"}}',
		'',
		'event: token',
		'data: {"delta":" — inclusion criteria found."}',
		'',
		'event: proposal_created',
		'data: {"tool":"propose_screening_decision","review_run_id":"' +
			reviewRunId +
			'","status_path":"/projects/project-1/review-runs/' +
			reviewRunId +
			'"}',
		'',
		'event: done',
		'data: {"message_id":"m-1","input_tokens":12,"output_tokens":34}',
		'',
		''
	].join('\n');
}

async function mockProjectShell(page: Page): Promise<void> {
	await page.route(`${api}/health/dependencies`, (route) =>
		route.fulfill({ json: dependencies })
	);
	await page.route(/\/api\/projects(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [project], next_cursor: null } })
	);
	await page.route(`${api}/projects/project-1`, (route) => route.fulfill({ json: project }));
	await page.route(/\/api\/projects\/project-1\/reports(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [], next_cursor: null } })
	);
	await page.route(/\/api\/ingestions(?:\?.*)?$/, (route) =>
		route.fulfill({ json: { items: [], next_cursor: null } })
	);
}

async function mockConversations(page: Page, items: Array<Record<string, unknown>>): Promise<void> {
	await page.route(`${api}/projects/project-1/assistant/conversations`, (route) =>
		route.fulfill({ json: items })
	);
}

async function openAssistant(page: Page): Promise<void> {
	await mockProjectShell(page);
	await mockConversations(page, [
		conversation(conversationId, 'Protocol questions', '2026-01-02T00:00:00Z'),
		conversation(secondConversationId, 'Duplicate checks', '2026-01-01T00:00:00Z')
	]);
	await page.route(
		`${api}/projects/project-1/assistant/conversations/${conversationId}/messages`,
		(route) =>
			route.fulfill({
				json: [
					{
						id: messageId,
						conversation_id: conversationId,
						role: 'user',
						content: 'What does the protocol say?',
						tool_calls: null,
						tool_results: null,
						metadata: null,
						created_at: '2026-01-01T00:00:00Z'
					},
					assistantMessage(`${messageId.slice(0, -1)}2`, '2026-01-01T00:00:01Z')
				]
			})
	);
	await page.goto('/projects/project-1/assistant');
}

test('opens the most recent thread and renders the persisted turn history', async ({ page }) => {
	await openAssistant(page);
	await expect(page.getByTestId('assistant-page')).toBeVisible();
	await expect(page.getByTestId('assistant-thread').first()).toContainText('Protocol questions');
	await expect(page.getByTestId('assistant-feed')).toContainText('What does the protocol say?');
	await expect(page.getByTestId('assistant-feed')).toContainText(
		'The protocol targets adults with type 2 diabetes.'
	);
	const toolCard = page.getByTestId('assistant-feed').locator('[data-slot="tool-call-card"]');
	await expect(toolCard).toContainText('get_project_protocol');
	await expect(toolCard).toContainText('Completed');
	const proposalCard = page.getByTestId('assistant-feed').locator('[data-slot="proposal-card"]');
	await expect(proposalCard).toContainText('Screening Decision');
	await expect(proposalCard.getByRole('link', { name: 'Review in Queue' })).toHaveAttribute(
		'href',
		'/projects/project-1/screening'
	);
	await expect(page.getByTestId('assistant-empty-state')).toHaveCount(0);
});

test('streams a new chat turn with tool progress, proposals, and token totals', async ({
	page
}) => {
	await openAssistant(page);
	await page.getByTestId('assistant-new-chat').click();
	await expect(page.getByTestId('assistant-empty-state')).toBeVisible();

	let createdConversation = false;
	await page.route(`${api}/projects/project-1/assistant/conversations`, async (route) => {
		if (route.request().method() === 'POST') {
			createdConversation = true;
			const body = route.request().postDataJSON() as { title: string };
			expect(body.title).toBe('What does the protocol say about inclusion criteria?');
			await route.fulfill({
				status: 201,
				json: conversation(conversationId, body.title, '2026-01-03T00:00:00Z')
			});
			return;
		}
		await route.fulfill({
			json: [
				conversation(conversationId, 'What does the protocol say', '2026-01-03T00:00:00Z')
			]
		});
	});
	await page.route(`${api}/projects/project-1/assistant/chat`, async (route) => {
		expect(route.request().method()).toBe('POST');
		expect(route.request().headers()['x-actor-kind']).toBe('user');
		expect(route.request().headers()['x-actor-id']).toBe('local-user');
		expect(route.request().postDataJSON()).toEqual({
			conversation_id: conversationId,
			message: 'What does the protocol say about inclusion criteria?'
		});
		await route.fulfill({
			status: 200,
			headers: { 'content-type': 'text/event-stream' },
			body: sseBody()
		});
	});

	await page
		.getByTestId('assistant-thread-input')
		.fill('What does the protocol say about inclusion criteria?');
	await page.getByTestId('assistant-send').click();

	const feed = page.getByTestId('assistant-feed');
	await expect(feed).toContainText('Reading the protocol — inclusion criteria found.');
	await expect(feed.locator('[data-slot="tool-call-card"]')).toContainText('Completed');
	await expect(feed.locator('[data-slot="proposal-card"]')).toContainText('Screening Decision');
	await expect(feed).toContainText('12 tokens in');
	await expect(createdConversation).toBe(true);
	await expect(page.getByTestId('assistant-thread')).toHaveCount(1);
});

test('shift+enter keeps a newline and enter submits the draft', async ({ page }) => {
	await openAssistant(page);
	await page.getByTestId('assistant-new-chat').click();
	const composer = page.getByTestId('assistant-thread-input');
	await composer.fill('first line');
	await composer.press('Shift+Enter');
	await expect(composer).toHaveValue('first line\n');
});

test('deletes a conversation after confirmation', async ({ page }) => {
	await openAssistant(page);
	page.once('dialog', (dialog) => dialog.accept());
	await page.route(
		`${api}/projects/project-1/assistant/conversations/${conversationId}`,
		(route) => {
			expect(route.request().method()).toBe('DELETE');
			return route.fulfill({ status: 204 });
		}
	);
	await page.getByTestId('assistant-thread').first().hover();
	await page.getByRole('button', { name: 'Delete conversation Protocol questions' }).click();
	await expect(page.getByTestId('assistant-thread')).toHaveCount(1);
});

test('keeps the assistant workspace within desktop and mobile bounds in dark mode', async ({
	page
}) => {
	await page.emulateMedia({ colorScheme: 'dark', reducedMotion: 'reduce' });
	for (const viewport of [
		{ width: 1440, height: 900 },
		{ width: 390, height: 844 }
	]) {
		await page.unrouteAll({ behavior: 'ignoreErrors' });
		await mockProjectShell(page);
		await mockConversations(page, []);
		await page.setViewportSize(viewport);
		await page.goto('/projects/project-1/assistant');
		await expect(page.getByTestId('assistant-page')).toBeVisible();
		const overflow = await page.evaluate(
			() => document.documentElement.scrollWidth > document.documentElement.clientWidth
		);
		expect(overflow, `unexpected horizontal overflow at ${viewport.width}px`).toBe(false);
	}
});
