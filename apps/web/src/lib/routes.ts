export type RouteMeta = {
	title: string;
	description: string;
};

const ROUTE_META: ReadonlyArray<readonly [string, RouteMeta]> = [
	[
		'/screening/full-text',
		{
			title: 'Full text screening · DeepRef',
			description: 'Review PDF evidence and screen included reports at full text.'
		}
	],
	[
		'/screening/title-abstract',
		{
			title: 'Title & abstract screening · DeepRef',
			description:
				'Protocol-grounded title and abstract screening for a DeepRef evidence workspace.'
		}
	],
	[
		'/discovery/duplicates',
		{
			title: 'Deduplication · DeepRef',
			description: 'Review duplicate-record proposals and preserve source-record provenance.'
		}
	],
	[
		'/deduplication',
		{
			title: 'Deduplication · DeepRef',
			description: 'Review duplicate-record proposals and preserve source-record provenance.'
		}
	],
	[
		'/appraisal',
		{ title: 'Appraisal · DeepRef', description: 'Assess study quality and risk of bias.' }
	],
	[
		'/extraction',
		{ title: 'Extraction · DeepRef', description: 'Capture structured findings from reports.' }
	],
	[
		'/automations',
		{ title: 'Automations · DeepRef', description: 'Schedule repeatable evidence workflows.' }
	],
	[
		'/assistant',
		{ title: 'Assistant · DeepRef', description: 'Ask questions with linked evidence.' }
	],
	[
		'/studies',
		{ title: 'Studies · DeepRef', description: 'Classify and group studies in the corpus.' }
	],
	[
		'/discovery/imports',
		{ title: 'Imports · DeepRef', description: 'Import source records and monitor runs.' }
	],
	[
		'/recommendations',
		{
			title: 'Recommendations · DeepRef',
			description: 'Review evidence-led article recommendations.'
		}
	],
	[
		'/articles',
		{ title: 'Articles · DeepRef', description: 'Browse the current evidence corpus.' }
	],
	[
		'/graph',
		{ title: 'Graph · DeepRef', description: 'Explore evidence relationships and overlays.' }
	],
	[
		'/prisma',
		{ title: 'PRISMA · DeepRef', description: 'Trace the flow of records through the review.' }
	],
	[
		'/protocol',
		{
			title: 'Protocol · DeepRef',
			description: 'Define eligibility, sources, and review rules.'
		}
	],
	[
		'/overview',
		{ title: 'Overview · DeepRef', description: 'A working summary of the evidence workspace.' }
	],
	[
		'/settings',
		{
			title: 'Settings · DeepRef',
			description: 'Application-level ingestion and Crossref parameters.'
		}
	]
];

function normalizePathname(pathname: string): string {
	return pathname.length > 1 ? pathname.replace(/\/+$/, '') : pathname;
}

function fallbackRouteMeta(pathname: string): RouteMeta {
	if (pathname === '/') {
		return {
			title: 'Evidence atelier · DeepRef',
			description: 'An auditable workspace for collecting, reviewing, and analyzing evidence.'
		};
	}
	return {
		title: 'DeepRef evidence workspace',
		description: 'An auditable workspace for collecting, reviewing, and analyzing evidence.'
	};
}

export function routeMetaForPathname(pathname: string): RouteMeta {
	const normalized = normalizePathname(pathname);
	return (
		ROUTE_META.find(([suffix]) => normalized.endsWith(suffix))?.[1] ??
		fallbackRouteMeta(normalized)
	);
}
