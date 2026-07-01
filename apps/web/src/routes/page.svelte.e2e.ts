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
		rank_score: 0.8
	}
];

async function mockWorkspace(page: Page) {
	await page.route('http://localhost:8080/projects', async (route) => {
		await route.fulfill({ json: [project] });
	});
	await page.route('http://localhost:8080/projects/test-project', async (route) => {
		await route.fulfill({ json: project });
	});
	await page.route('http://localhost:8080/projects/test-project/articles', async (route) => {
		await route.fulfill({ json: articles });
	});
	await page.route('http://localhost:8080/projects/test-project/graph', async (route) => {
		await route.fulfill({
			json: {
				nodes: articles,
				edges: [{ source: '10.1/source', target: '10.1/source' }]
			}
		});
	});
	await page.route(
		'http://localhost:8080/projects/test-project/recommendations',
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
		'http://localhost:8080/projects/test-project/articles/MTAuMS9zb3VyY2U',
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
	await page.route('http://localhost:8080/ingestions', async (route) => {
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
	await page.route('http://localhost:8080/ingestions/new-ingestion', async (route) => {
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
	await page.route('http://localhost:8080/ingestions/new-ingestion/items', async (route) => {
		await route.fulfill({ json: [] });
	});
}

async function mockProjectCreateWorkspace(page: Page, initialProjects = [project]) {
	let projects = [...initialProjects];

	await page.route('http://localhost:8080/projects', async (route) => {
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
		await page.route(`http://localhost:8080/projects/${mockedProject.id}`, async (route) => {
			await route.fulfill({ json: mockedProject });
		});
		await page.route(
			`http://localhost:8080/projects/${mockedProject.id}/articles`,
			async (route) => {
				await route.fulfill({ json: mockedProject.id === project.id ? articles : [] });
			}
		);
		await page.route(
			`http://localhost:8080/projects/${mockedProject.id}/graph`,
			async (route) => {
				await route.fulfill({ json: { nodes: [], edges: [] } });
			}
		);
		await page.route(
			`http://localhost:8080/projects/${mockedProject.id}/recommendations`,
			async (route) => {
				await route.fulfill({
					json: { foundational: [], core_to_project: [], underexplored: [] }
				});
			}
		);
	}

	await page.route('http://localhost:8080/ingestions', async (route) => {
		await route.fulfill({ json: [] });
	});
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

	await page.getByRole('button', { name: 'Articles' }).click();
	await page.getByRole('button', { name: 'Open' }).first().click();
	await expect(page.getByText('A useful article abstract.')).toBeVisible();
});

test('secondary article views reuse selected article inspector', async ({ page }) => {
	await mockWorkspace(page);
	await page.goto('/');

	await page.getByRole('button', { name: 'Articles' }).click();
	await page.getByRole('button', { name: 'Open' }).first().click();
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

test('ingestions are filtered and create uses current project', async ({ page }) => {
	await mockWorkspace(page);
	await page.goto('/');

	await page.getByRole('button', { name: 'Ingestions' }).click();
	await expect(page.getByText('1 project runs')).toBeVisible();
	await expect(page.getByText('other-ingestion')).toHaveCount(0);
	await page.locator('#dois').fill('10.1/new');
	await page.getByRole('button', { name: 'Start ingestion' }).click();
	await expect(page.getByText('new-ingestion')).toBeVisible();
	await expect(page.getByText('Status')).toBeVisible();
	await expect(page.getByText('queued')).toBeVisible();
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
