import type { LucideIcon } from '@lucide/svelte';
import { resolve } from '$app/paths';
import type { ProjectWorkspaceNavView } from './types';
import ArchiveIcon from '@lucide/svelte/icons/archive';
import BotIcon from '@lucide/svelte/icons/bot';
import ClipboardCheckIcon from '@lucide/svelte/icons/clipboard-check';
import ClipboardListIcon from '@lucide/svelte/icons/clipboard-list';
import ClipboardPenLineIcon from '@lucide/svelte/icons/clipboard-pen-line';
import FileTextIcon from '@lucide/svelte/icons/file-text';
import GitCompareIcon from '@lucide/svelte/icons/git-compare';
import GitForkIcon from '@lucide/svelte/icons/git-fork';
import HomeIcon from '@lucide/svelte/icons/home';
import LightbulbIcon from '@lucide/svelte/icons/lightbulb';
import Settings2Icon from '@lucide/svelte/icons/settings-2';
import TablePropertiesIcon from '@lucide/svelte/icons/table-properties';

export type NavigationGroupId = 'plan' | 'collect' | 'review' | 'analyze' | 'operate';

export type ProjectRoute =
	| '/projects/[projectId]/overview'
	| '/projects/[projectId]/protocol'
	| '/projects/[projectId]/prisma'
	| '/projects/[projectId]/studies'
	| '/projects/[projectId]/discovery/imports'
	| '/projects/[projectId]/articles'
	| '/projects/[projectId]/screening/title-abstract'
	| '/projects/[projectId]/screening/full-text'
	| '/projects/[projectId]/appraisal'
	| '/projects/[projectId]/discovery/duplicates'
	| '/projects/[projectId]/graph'
	| '/projects/[projectId]/recommendations'
	| '/projects/[projectId]/extraction'
	| '/projects/[projectId]/automations'
	| '/projects/[projectId]/assistant'
	| '/projects/[projectId]/deduplication';

export type ProjectNavigationItem = {
	id: string;
	label: string;
	description: string;
	path: ProjectRoute;
	aliases?: readonly ProjectRoute[];
	icon: LucideIcon;
	view?: ProjectWorkspaceNavView;
};

export type ProjectNavigationGroup = {
	id: NavigationGroupId;
	label: string;
	description: string;
	items: readonly ProjectNavigationItem[];
};

export const PROJECT_NAVIGATION_GROUPS: readonly ProjectNavigationGroup[] = [
	{
		id: 'plan',
		label: 'Plan',
		description: 'Shape the review question and protocol.',
		items: [
			{
				id: 'overview',
				label: 'Overview',
				description: 'A working summary of the evidence workspace.',
				path: '/projects/[projectId]/overview',
				icon: HomeIcon,
				view: 'overview'
			},
			{
				id: 'protocol',
				label: 'Protocol',
				description: 'Define eligibility, sources, and review rules.',
				path: '/projects/[projectId]/protocol',
				icon: ClipboardListIcon,
				view: 'protocol'
			}
		]
	},
	{
		id: 'collect',
		label: 'Collect',
		description: 'Bring source records and evidence together.',
		items: [
			{
				id: 'imports',
				label: 'Imports',
				description: 'Ingest source records and monitor runs.',
				path: '/projects/[projectId]/discovery/imports',
				icon: ArchiveIcon,
				view: 'ingestions'
			},
			{
				id: 'articles',
				label: 'Articles',
				description: 'Browse the current evidence corpus.',
				path: '/projects/[projectId]/articles',
				icon: FileTextIcon,
				view: 'articles'
			},
			{
				id: 'deduplication',
				label: 'Deduplication',
				description: 'Resolve duplicate records with provenance.',
				path: '/projects/[projectId]/discovery/duplicates',
				aliases: ['/projects/[projectId]/deduplication'],
				icon: GitCompareIcon
			}
		]
	},
	{
		id: 'review',
		label: 'Review',
		description: 'Make auditable decisions about the evidence.',
		items: [
			{
				id: 'title-abstract-screening',
				label: 'Title & abstract',
				description: 'Screen records against the protocol.',
				path: '/projects/[projectId]/screening/title-abstract',
				icon: ClipboardCheckIcon
			},
			{
				id: 'full-text-screening',
				label: 'Full text',
				description: 'Review included reports at full text.',
				path: '/projects/[projectId]/screening/full-text',
				icon: FileTextIcon
			},
			{
				id: 'studies',
				label: 'Studies',
				description: 'Classify and group studies in the corpus.',
				path: '/projects/[projectId]/studies',
				icon: ClipboardListIcon
			},
			{
				id: 'appraisal',
				label: 'Appraisal',
				description: 'Assess study quality and risk of bias.',
				path: '/projects/[projectId]/appraisal',
				icon: ClipboardPenLineIcon
			},
			{
				id: 'extraction',
				label: 'Extraction',
				description: 'Capture structured findings from reports.',
				path: '/projects/[projectId]/extraction',
				icon: TablePropertiesIcon
			}
		]
	},
	{
		id: 'analyze',
		label: 'Analyze',
		description: 'Explore relationships and extract findings.',
		items: [
			{
				id: 'prisma',
				label: 'PRISMA',
				description: 'Trace the flow of records through the review.',
				path: '/projects/[projectId]/prisma',
				icon: ClipboardCheckIcon,
				view: 'prisma'
			},
			{
				id: 'graph',
				label: 'Graph',
				description: 'Explore evidence relationships and overlays.',
				path: '/projects/[projectId]/graph',
				icon: GitForkIcon,
				view: 'graph'
			},
			{
				id: 'recommendations',
				label: 'Recommendations',
				description: 'Review evidence-led article recommendations.',
				path: '/projects/[projectId]/recommendations',
				icon: LightbulbIcon,
				view: 'recommendations'
			}
		]
	},
	{
		id: 'operate',
		label: 'Operate',
		description: 'Automate the work and keep it explainable.',
		items: [
			{
				id: 'automations',
				label: 'Automations',
				description: 'Schedule repeatable evidence workflows.',
				path: '/projects/[projectId]/automations',
				icon: Settings2Icon
			},
			{
				id: 'assistant',
				label: 'Assistant',
				description: 'Ask questions with linked evidence.',
				path: '/projects/[projectId]/assistant',
				icon: BotIcon
			}
		]
	}
] as const;

export function isProjectNavigationItemActive(
	item: ProjectNavigationItem,
	pathname: string,
	projectId: string
): boolean {
	const paths = [item.path, ...(item.aliases ?? [])];
	return paths.some((path) => {
		const resolved = resolve(path, { projectId });
		return pathname === resolved || pathname.startsWith(`${resolved}/`);
	});
}
