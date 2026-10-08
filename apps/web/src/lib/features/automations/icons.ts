import type { Component } from 'svelte';
import BellIcon from '@lucide/svelte/icons/bell';
import BotIcon from '@lucide/svelte/icons/bot';
import ClockIcon from '@lucide/svelte/icons/clock';
import DatabaseIcon from '@lucide/svelte/icons/database';
import DownloadIcon from '@lucide/svelte/icons/download';
import FileDownIcon from '@lucide/svelte/icons/file-down';
import FilterIcon from '@lucide/svelte/icons/filter';
import FlagIcon from '@lucide/svelte/icons/flag';
import GitBranchIcon from '@lucide/svelte/icons/git-branch';
import GlobeIcon from '@lucide/svelte/icons/globe';
import HourglassIcon from '@lucide/svelte/icons/hourglass';
import LayersIcon from '@lucide/svelte/icons/layers';
import ListChecksIcon from '@lucide/svelte/icons/list-checks';
import MailIcon from '@lucide/svelte/icons/mail';
import MergeIcon from '@lucide/svelte/icons/merge';
import MousePointerClickIcon from '@lucide/svelte/icons/mouse-pointer-click';
import PlugIcon from '@lucide/svelte/icons/plug';
import RepeatIcon from '@lucide/svelte/icons/repeat';
import RssIcon from '@lucide/svelte/icons/rss';
import SearchIcon from '@lucide/svelte/icons/search';
import SendIcon from '@lucide/svelte/icons/send';
import SparklesIcon from '@lucide/svelte/icons/sparkles';
import WebhookIcon from '@lucide/svelte/icons/webhook';
import ZapIcon from '@lucide/svelte/icons/zap';

type Icon = Component<{ class?: string }>;

const BY_NODE: Record<string, Icon> = {
	'trigger.manual': MousePointerClickIcon,
	'trigger.schedule': ClockIcon,
	'trigger.webhook': WebhookIcon,
	'trigger.publication_alert': RssIcon,
	'trigger.email': MailIcon,
	'data.find_records': SearchIcon,
	'data.get_report': DatabaseIcon,
	'data.get_study': LayersIcon,
	'data.screening_queue': ListChecksIcon,
	'action.import_identifiers': DownloadIcon,
	'action.record_decision': FlagIcon,
	'action.group_into_study': LayersIcon,
	'action.attach_pdf': FileDownIcon,
	'action.export': FileDownIcon,
	'ai.prompt': SparklesIcon,
	'ai.classify': BotIcon,
	'logic.if': GitBranchIcon,
	'logic.filter': FilterIcon,
	'logic.for_each': RepeatIcon,
	'logic.wait': HourglassIcon,
	'logic.merge': MergeIcon,
	'integration.http_request': GlobeIcon,
	'integration.notify': BellIcon,
	'integration.email': MailIcon,
	'integration.slack': SendIcon
};

const BY_CATEGORY: Record<string, Icon> = {
	trigger: ZapIcon,
	data: DatabaseIcon,
	action: FlagIcon,
	ai: SparklesIcon,
	logic: GitBranchIcon,
	integration: PlugIcon
};

export function iconForNode(typeId: string, category: string | undefined): Icon {
	return BY_NODE[typeId] ?? BY_CATEGORY[category ?? ''] ?? ZapIcon;
}

export function iconForCategory(category: string): Icon {
	return BY_CATEGORY[category] ?? ZapIcon;
}
