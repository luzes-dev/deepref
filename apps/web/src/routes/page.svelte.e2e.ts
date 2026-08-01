import { expect, test, type Page } from '@playwright/test';

const project = {
	id: 'test-project',
	name: 'Test Project',
	description: 'A mocked project',
	default_max_depth: 2,
	created_at: '2026-01-01T00:00:00Z',
	updated_at: '2026-01-01T00:00:00Z'
};

const createdProject = {
	id: 'created-project',
	name: 'Created Project',
	description: 'Created from test',
	default_max_depth: 2,
	created_at: '2026-01-02T00:00:00Z',
	updated_at: '2026-01-02T00:00:00Z'
};

const secondaryProject = {
	id: 'secondary-project',
	name: 'Secondary Project',
	description: 'Another mocked project',
	default_max_depth: 3,
	created_at: '2026-01-03T00:00:00Z',
	updated_at: '2026-01-03T00:00:00Z'
};

const articles = [
	{
		doi: '10.1/source',
		doi_key: 'MTAuMS9zb3VyY2U',
		title: 'Source Article',
		issued_year: 2024,
		type: 'article',
		total_citations: 10,
		internal_citations: 2,
		outbound_internal_references: 1,
		rank_score: 0.99
	},
	{
		doi: '10.2/review',
		doi_key: 'MTAuMi9yZXZpZXc',
		title: 'Review Article',
		issued_year: 2022,
		type: 'review',
		total_citations: 22,
		internal_citations: 4,
		outbound_internal_references: 2,
		rank_score: 0.74
	},
	{
		doi: '10.3/archive',
		doi_key: 'MTAuMy9hcmNoaXZl',
		title: 'Archive Article',
		issued_year: 2018,
		type: 'article',
		total_citations: 5,
		internal_citations: 1,
		outbound_internal_references: 0,
		rank_score: 0.12
	}
];

const paginatedArticles = Array.from({ length: 9 }, (_, index) => ({
	doi: `10.4/page-${index + 1}`,
	doi_key: `MTAuNC9wYWdlLTE${index}`,
	title: `Pagination Article ${index + 1}`,
	issued_year: 2020 + (index % 4),
	type: index % 2 === 0 ? 'article' : 'preprint',
	total_citations: index + 1,
	internal_citations: index % 3,
	outbound_internal_references: index % 2,
	rank_score: 0.65 - index * 0.03
}));

const workspaceArticles = [...articles, ...paginatedArticles];

async function mockWorkspace(page: Page) {
	await page.route('http://localhost:4173/api/projects', async (route) => {
		await route.fulfill({ json: [project] });
	});
	await page.route('http://localhost:4173/api/projects/test-project', async (route) => {
		await route.fulfill({ json: project });
	});
	await page.route('http://localhost:4173/api/projects/test-project/articles', async (route) => {
		await route.fulfill({ json: workspaceArticles });
	});
	await page.route('http://localhost:4173/api/projects/test-project/graph', async (route) => {
		await route.fulfill({
			json: {
				nodes: workspaceArticles,
				edges: [{ source: '10.1/source', target: '10.1/source' }]
			}
		});
	});
	await page.route(
		'http://localhost:4173/api/projects/test-project/recommendations',
		async (route) => {
			await route.fulfill({
				json: {
					foundational: [],
					core_to_project: [],
					underexplored: []
				}
			});
		}
	);
	await page.route(
		'http://localhost:4173/api/projects/test-project/articles/MTAuMS9zb3VyY2U',
		async (route) => {
			await route.fulfill({
				json: {
					...articles[0],
					abstract: 'A useful article abstract.',
					container_title: 'Journal',
					publisher: 'Publisher',
					published_year: 2024,
					references_count: 3,
					raw: { DOI: '10.1/source' },
					url: null
				}
			});
		}
	);
	await page.route('http://localhost:4173/api/ingestions', async (route) => {
		if (route.request().method() === 'POST') {
			const body = route.request().postDataJSON();
			expect(body).toMatchObject({
				project_id: 'test-project',
				seed_dois: ['10.1/new'],
				metadata_provider: 'crossref',
				citation_provider: 'crossref',
				max_depth: 2
			});
			await route.fulfill({
				status: 201,
				json: {
					id: 'new-ingestion',
					project_id: 'test-project',
					status: 'queued',
					seed_count: 1,
					fetched_count: 0,
					failed_count: 0,
					queued_count: 1,
					max_depth: 2,
					created_at: '2026-01-01T00:00:00Z',
					started_at: null,
					completed_at: null
				}
			});
			return;
		}
		await route.fulfill({
			json: [
				{
					id: 'project-ingestion',
					project_id: 'test-project',
					status: 'completed',
					seed_count: 1,
					fetched_count: 1,
					failed_count: 0,
					queued_count: 0,
					max_depth: 2,
					created_at: '2026-01-01T00:00:00Z',
					started_at: null,
					completed_at: '2026-01-01T00:01:00Z'
				},
				{
					id: 'other-ingestion',
					project_id: 'other-project',
					status: 'completed',
					seed_count: 9,
					fetched_count: 9,
					failed_count: 0,
					queued_count: 0,
					max_depth: 2,
					created_at: '2026-01-01T00:00:00Z',
					started_at: null,
					completed_at: '2026-01-01T00:01:00Z'
				}
			]
		});
	});
	await page.route('http://localhost:4173/api/ingestions/new-ingestion', async (route) => {
		await route.fulfill({
			json: {
				id: 'new-ingestion',
				project_id: 'test-project',
				status: 'queued',
				seed_count: 1,
				fetched_count: 0,
				failed_count: 0,
				queued_count: 1,
				max_depth: 2,
				created_at: '2026-01-01T00:00:00Z',
				started_at: null,
				completed_at: null
			}
		});
	});
	await page.route('http://localhost:4173/api/ingestions/new-ingestion/items', async (route) => {
		await route.fulfill({ json: [] });
	});
}

async function mockProjectCreateWorkspace(page: Page, initialProjects = [project]) {
	let projects = [...initialProjects];

	await page.route('http://localhost:4173/api/projects', async (route) => {
		if (route.request().method() === 'POST') {
			const body = route.request().postDataJSON();
			expect(body).toMatchObject({
				name: 'Created Project',
				description: 'Created from test',
				default_max_depth: 2
			});
			projects = [...projects, createdProject];
			await route.fulfill({ status: 201, json: createdProject });
			return;
		}
		await route.fulfill({ json: projects });
	});

	for (const mockedProject of [project, createdProject]) {
		await page.route(
			`http://localhost:4173/api/projects/${mockedProject.id}`,
			async (route) => {
				await route.fulfill({ json: mockedProject });
			}
		);
		await page.route(
			`http://localhost:4173/api/projects/${mockedProject.id}/articles`,
			async (route) => {
				await route.fulfill({ json: mockedProject.id === project.id ? articles : [] });
			}
		);
		await page.route(
			`http://localhost:4173/api/projects/${mockedProject.id}/graph`,
			async (route) => {
				await route.fulfill({ json: { nodes: [], edges: [] } });
			}
		);
		await page.route(
			`http://localhost:4173/api/projects/${mockedProject.id}/recommendations`,
			async (route) => {
				await route.fulfill({
					json: { foundational: [], core_to_project: [], underexplored: [] }
				});
			}
		);
	}

	await page.route('http://localhost:4173/api/ingestions', async (route) => {
		await route.fulfill({ json: [] });
	});
}

async function mockProjectManagementWorkspace(
	page: Page,
	initialProjects = [project, secondaryProject]
) {
	let projects = [...initialProjects];

	await page.route('http://localhost:4173/api/projects', async (route) => {
		await route.fulfill({ json: projects });
	});

	await page.route(/http:\/\/localhost:4173\/api\/projects\/[^/]+(?:\/.*)?$/, async (route) => {
		const request = route.request();
		const url = new URL(request.url());
		const [, , , projectId, resource] = url.pathname.split('/');
		const mockedProject = projects.find((candidate) => candidate.id === projectId);

		if (!mockedProject) {
			await route.fulfill({ status: 404, json: { message: 'Project not found' } });
			return;
		}

		if (resource === 'articles') {
			await route.fulfill({ json: projectId === project.id ? articles : [] });
			return;
		}

		if (resource === 'graph') {
			await route.fulfill({ json: { nodes: [], edges: [] } });
			return;
		}

		if (resource === 'recommendations') {
			await route.fulfill({
				json: { foundational: [], core_to_project: [], underexplored: [] }
			});
			return;
		}

		if (request.method() === 'PATCH') {
			const body = request.postDataJSON();
			expect(body).toMatchObject({
				name: 'Renamed Project',
				description: 'Updated from management',
				default_max_depth: project.default_max_depth
			});
			projects = projects.map((candidate) =>
				candidate.id === projectId
					? {
							...candidate,
							name: body.name,
							description: body.description,
							default_max_depth: body.default_max_depth,
							updated_at: '2026-01-04T00:00:00Z'
						}
					: candidate
			);
			await route.fulfill({
				status: 200,
				json: projects.find((candidate) => candidate.id === projectId)
			});
			return;
		}

		if (request.method() === 'DELETE') {
			projects = projects.filter((candidate) => candidate.id !== projectId);
			await route.fulfill({ status: 204, body: '' });
			return;
		}

		await route.fulfill({ json: mockedProject });
	});

	await page.route('http://localhost:4173/api/ingestions', async (route) => {
		await route.fulfill({ json: [] });
	});
}

async function openSourceArticle(page: Page) {
	await page.getByRole('button', { name: 'Articles' }).click();
	await page.getByRole('button', { name: /Source Article/ }).click();
}

test('renders unified workspace without global ingestion links', async ({ page }) => {
	await mockWorkspace(page);
	await page.goto('/');

	await expect(page.getByRole('heading', { name: 'Test Project' })).toBeVisible();
	await expect(page.getByRole('link', { name: 'Ingestions' })).toHaveCount(0);
	await expect(page.getByRole('link', { name: 'Ingest' })).toHaveCount(0);
});

test('selecting an article shows inspector', async ({ page }) => {
	await mockWorkspace(page);
	await page.goto('/');

	await openSourceArticle(page);
	await expect(page.getByText('A useful article abstract.')).toBeVisible();
});

test('sidebar keeps its size and remains collapsible when the inspector appears', async ({
	page
}) => {
	await mockWorkspace(page);
	await page.goto('/');

	const sidebarPane = page.locator('[data-pane]').first();
	const sidebarHandle = page.locator('[data-pane-resizer]').first();

	const collapseSidebar = async () => {
		await sidebarHandle.focus();
		await sidebarHandle.press('Enter');
		await expect
			.poll(async () => (await sidebarPane.boundingBox())?.width ?? Infinity)
			.toBeLessThan(80);
	};

	await collapseSidebar();
	await page.getByRole('button', { name: 'Articles' }).click();
	await expect(page.getByRole('heading', { name: 'Article inspector' })).toBeVisible();
	await expect
		.poll(async () => (await sidebarPane.boundingBox())?.width ?? Infinity)
		.toBeLessThan(80);

	await sidebarHandle.focus();
	await sidebarHandle.press('Enter');
	await expect
		.poll(async () => (await sidebarPane.boundingBox())?.width ?? 0)
		.toBeGreaterThan(150);

	await page.waitForTimeout(350);
	const expandedHandle = await sidebarHandle.boundingBox();
	if (!expandedHandle) throw new Error('Expanded sidebar resize handle is not visible');
	await page.mouse.move(
		expandedHandle.x + expandedHandle.width / 2,
		expandedHandle.y + expandedHandle.height / 2
	);
	await page.mouse.down();
	await page.mouse.move(20, expandedHandle.y + expandedHandle.height / 2, { steps: 5 });
	await page.mouse.up();
	await expect
		.poll(async () => (await sidebarPane.boundingBox())?.width ?? Infinity)
		.toBeLessThan(80);
});

test('secondary article views reuse selected article inspector', async ({ page }) => {
	await mockWorkspace(page);
	await page.goto('/');

	await openSourceArticle(page);
	await expect(page.getByText('A useful article abstract.')).toBeVisible();

	await page.getByRole('button', { name: 'Recommendations' }).click();
	await expect(page.getByText('A useful article abstract.')).toBeVisible();

	await page.getByRole('button', { name: 'Ingestions' }).click();
	await expect(page.getByText('1 project runs')).toBeVisible();

	await page.getByRole('button', { name: 'Graph' }).click();
	await expect(page.getByText('A useful article abstract.')).toBeVisible();
	await expect(page.getByText('Matches')).toHaveCount(0);
	await expect(page.getByRole('button', { name: 'Reset graph layout' })).toBeVisible();
});

test('article table filters, resets, sorts, and paginates', async ({ page }) => {
	await mockWorkspace(page);
	await page.goto('/');

	await page.getByRole('button', { name: 'Articles' }).click();
	await expect(page.getByText('Page 1 of 2')).toBeVisible();

	await page.getByRole('textbox', { name: 'Search title or DOI' }).fill('review');
	await expect(page.getByRole('button', { name: /Review Article/ })).toBeVisible();
	await expect(page.getByRole('button', { name: /Source Article/ })).toHaveCount(0);

	await page.getByRole('button', { name: /Reset/ }).click();
	await expect(page.getByRole('button', { name: /Source Article/ })).toBeVisible();

	await page
		.getByRole('columnheader', { name: /Year/ })
		.getByRole('button', { name: /Year/ })
		.click();
	await page.getByRole('menuitem', { name: 'Asc' }).click();
	await expect(
		page
			.locator('tbody tr')
			.first()
			.getByRole('button', { name: /Archive Article/ })
	).toBeVisible();

	await page.getByRole('button', { name: 'Go to next page' }).click();
	await expect(page.getByText('Page 2 of 2')).toBeVisible();
});

test('mobile articles keep card open action', async ({ page }) => {
	await page.setViewportSize({ width: 390, height: 844 });
	await mockWorkspace(page);
	await page.goto('/');

	await page.getByRole('tab', { name: 'Articles' }).click();
	await expect(page.getByRole('button', { name: 'Open' }).first()).toBeVisible();
});

test('ingestions are filtered and create uses current project', async ({ page }) => {
	await mockWorkspace(page);
	await page.goto('/');

	await page.getByRole('button', { name: 'Ingestions' }).click();
	await expect(page.getByText('1 project runs')).toBeVisible();
	await expect(page.getByText('other-ingestion')).toHaveCount(0);
	await page.locator('#dois').fill('10.1/new');
	await page.getByRole('button', { name: 'Start ingestion' }).click();
	await expect(page.getByText('new-ingestion')).toBeVisible();
	await expect(page.getByRole('heading', { name: 'Status queued' })).toBeVisible();
	await expect(page.getByText('queued', { exact: true }).first()).toBeVisible();
});

test('empty workspace creates a first project', async ({ page }) => {
	await mockProjectCreateWorkspace(page, []);
	await page.goto('/');

	await page.getByRole('button', { name: 'Create project' }).click();
	await page.locator('#empty-project-name').fill('Created Project');
	await page.locator('#empty-project-description').fill('Created from test');
	await page.getByRole('button', { name: 'Create', exact: true }).click();

	await expect(page.getByRole('heading', { name: 'Created Project' })).toBeVisible();
	await expect(page.getByRole('combobox', { name: 'Select project' })).toContainText(
		'Created Project'
	);
});

test('selector create path creates and selects a project', async ({ page }) => {
	await mockProjectCreateWorkspace(page);
	await page.goto('/');

	await page.getByRole('combobox', { name: 'Select project' }).click();
	await page.getByText('Create project').click();
	await page.locator('#selector-project-name').fill('Created Project');
	await page.locator('#selector-project-description').fill('Created from test');
	await page.getByRole('button', { name: 'Create', exact: true }).click();

	await expect(page.getByRole('heading', { name: 'Created Project' })).toBeVisible();
	await expect(page.getByRole('combobox', { name: 'Select project' })).toContainText(
		'Created Project'
	);
});

test('selector management edits and deletes projects', async ({ page }) => {
	await mockProjectManagementWorkspace(page);
	await page.goto('/');

	await page.getByRole('combobox', { name: 'Select project' }).click();
	const createItem = page.getByText('Create project', { exact: true });
	const manageItem = page.getByText('Manage projects', { exact: true });
	await expect(createItem).toBeVisible();
	await expect(manageItem).toBeVisible();
	const createBox = await createItem.boundingBox();
	const manageBox = await manageItem.boundingBox();
	expect(createBox?.y).toBeLessThan(manageBox?.y ?? 0);

	await manageItem.click();
	await expect(page.getByRole('heading', { name: 'Manage projects' })).toBeVisible();

	await page.locator('#management-project-name-test-project').fill('Renamed Project');
	await page
		.locator('#management-project-description-test-project')
		.fill('Updated from management');
	await page
		.locator('fieldset')
		.filter({ has: page.locator('#management-project-name-test-project') })
		.getByRole('button', { name: 'Save changes' })
		.click();
	await expect(page.getByRole('heading', { name: 'Renamed Project' })).toBeVisible();
	await expect(page.getByRole('combobox', { name: 'Select project' })).toContainText(
		'Renamed Project'
	);

	await page
		.locator('fieldset')
		.filter({ hasText: 'Secondary Project' })
		.getByRole('button', { name: 'Delete' })
		.click();
	await expect(page.getByRole('button', { name: 'Confirm delete' })).toBeVisible();
	await page.getByRole('button', { name: 'Cancel' }).click();
	await expect(page.locator('fieldset').filter({ hasText: 'Secondary Project' })).toBeVisible();

	await page
		.locator('fieldset')
		.filter({ hasText: 'Renamed Project' })
		.getByRole('button', { name: 'Delete' })
		.click();
	await page.getByRole('button', { name: 'Confirm delete' }).click();
	await expect(page.locator('fieldset').filter({ hasText: 'Renamed Project' })).toHaveCount(0);
	await expect(page.getByRole('heading', { name: 'Secondary Project' })).toBeVisible();
	await expect(page.getByRole('combobox', { name: 'Select project' })).toContainText(
		'Secondary Project'
	);
});

test('mobile project management modal is padded and scrollable', async ({ page }) => {
	await page.setViewportSize({ width: 390, height: 844 });
	const manyProjects = Array.from({ length: 8 }, (_, index) => ({
		...project,
		id: `mobile-project-${index + 1}`,
		name: `Mobile Project ${index + 1}`,
		description: `Mobile project ${index + 1}`,
		created_at: `2026-01-${String(index + 1).padStart(2, '0')}T00:00:00Z`,
		updated_at: `2026-01-${String(index + 1).padStart(2, '0')}T00:00:00Z`
	}));
	await mockProjectManagementWorkspace(page, manyProjects);
	await page.goto('/');

	await page.getByRole('combobox', { name: 'Select project' }).click();
	await page.getByText('Manage projects', { exact: true }).click();
	await expect(page.getByRole('heading', { name: 'Manage projects' })).toBeVisible();

	const drawer = page.getByRole('dialog', { name: 'Manage projects' });
	const firstProject = page.locator('fieldset').filter({ hasText: 'Mobile Project 1' });
	const drawerBox = await drawer.boundingBox();
	const firstProjectBox = await firstProject.boundingBox();
	expect(firstProjectBox?.x ?? 0).toBeGreaterThan((drawerBox?.x ?? 0) + 8);

	const viewport = drawer.locator('[data-slot="scroll-area-viewport"]');
	const canScroll = await viewport.evaluate(
		(element) => element.scrollHeight > element.clientHeight
	);
	expect(canScroll).toBe(true);
	await viewport.evaluate((element) => {
		element.scrollTop = element.scrollHeight;
	});

	await expect(page.locator('fieldset').filter({ hasText: 'Mobile Project 8' })).toBeVisible();
});

test('desktop project management modal contains long lists', async ({ page }) => {
	const manyProjects = Array.from({ length: 10 }, (_, index) => ({
		...project,
		id: `desktop-project-${index + 1}`,
		name: `Desktop Project ${index + 1}`,
		description: `Desktop project ${index + 1}`,
		created_at: `2026-02-${String(index + 1).padStart(2, '0')}T00:00:00Z`,
		updated_at: `2026-02-${String(index + 1).padStart(2, '0')}T00:00:00Z`
	}));
	await mockProjectManagementWorkspace(page, manyProjects);
	await page.goto('/');

	await page.getByRole('combobox', { name: 'Select project' }).click();
	await page.getByText('Manage projects', { exact: true }).click();
	await expect(page.getByRole('heading', { name: 'Manage projects' })).toBeVisible();

	const dialog = page.getByRole('dialog', { name: 'Manage projects' });
	const viewport = dialog.locator('[data-slot="scroll-area-viewport"]');
	const dialogBox = await dialog.boundingBox();
	const viewportBox = await viewport.boundingBox();
	expect((viewportBox?.y ?? 0) + (viewportBox?.height ?? 0)).toBeLessThanOrEqual(
		(dialogBox?.y ?? 0) + (dialogBox?.height ?? 0)
	);
	const canScroll = await viewport.evaluate(
		(element) => element.scrollHeight > element.clientHeight
	);
	expect(canScroll).toBe(true);
});
