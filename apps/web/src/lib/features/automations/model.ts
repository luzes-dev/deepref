/**
 * Pure helpers behind the automation builder: the plain-language copy for
 * blocks, port compatibility, schedule text and the mapping between the
 * API graph and the canvas.
 */
import type { BuilderNodeData, BuilderPort, BuilderTone } from '@deepref/ui/flow';
import type {
	ConfigFieldDto,
	NodeTypeDto,
	PortDto,
	WorkflowGraphDto,
	WorkflowGraphEdgeDto,
	WorkflowGraphNodeDto
} from '#lib/api/generated/models/index.js';
import type { FlowGraphEdge as Edge, FlowGraphNode as Node } from '@deepref/ui/flow';

export type WfNode = WorkflowGraphNodeDto;
export type WfEdge = WorkflowGraphEdgeDto;
export type WfGraph = WorkflowGraphDto;

export type Category = 'trigger' | 'data' | 'action' | 'ai' | 'logic' | 'integration';

export const CATEGORY_ORDER: Category[] = [
	'trigger',
	'data',
	'ai',
	'action',
	'logic',
	'integration'
];

export const CATEGORY_LABEL: Record<Category, string> = {
	trigger: 'When this happens',
	data: 'Get data',
	action: 'Do something',
	ai: 'AI',
	logic: 'Decide and repeat',
	integration: 'Connect other tools'
};

/** Everyday words people may type when looking for a block. */
export const BLOCK_ALIASES: Record<string, string> = {
	'logic.filter': 'filter narrow only keep remove',
	'logic.if': 'condition branch decide choose yes no else',
	'logic.for_each': 'loop repeat each iterate every',
	'logic.wait': 'delay pause sleep later timer',
	'logic.merge': 'join combine gather together',
	'integration.http_request': 'api request http webhook post get call url',
	'integration.notify': 'alert message bell notification',
	'integration.email': 'mail send message',
	'integration.slack': 'chat message channel post',
	'ai.prompt': 'ask question gpt llm chat custom prompt',
	'ai.classify': 'screen include exclude relevance',
	'ai.extract_field': 'extract fill data',
	'trigger.schedule': 'timer cron daily weekly monthly repeat',
	'trigger.webhook': 'http api url call',
	'trigger.publication_alert': 'pubmed crossref search alert new papers literature'
};

export function isCategory(value: string): value is Category {
	return (CATEGORY_ORDER as string[]).includes(value);
}

export function toneOf(category: string): BuilderTone {
	return isCategory(category) ? category : 'action';
}

// ---------------------------------------------------------------------------
// Ports

const PORT_NAME: Record<string, string> = {
	trigger: 'a start signal',
	records: 'a list of records',
	record: 'one record',
	report: 'one record',
	study: 'a study',
	document: 'a full text',
	text: 'text',
	json: 'general information'
};

export function portName(type: string): string {
	return PORT_NAME[type] ?? 'information';
}

/** Mirrors `PortType::connects_to` on the server. */
export function portsCompatible(from: string, to: string): boolean {
	if (from === to) return true;
	if (to === 'json') return true;
	const single = ['record', 'report'];
	if (single.includes(from) && (to === 'records' || single.includes(to))) return true;
	if ((from === 'records' || single.includes(from)) && to === 'text') return true;
	return false;
}

/** Plain-language reason shown when two ports refuse to connect. */
export function refusalReason(from: PortDto, to: PortDto): string {
	if (from.type === 'records' && (to.type === 'record' || to.type === 'report')) {
		return 'This step gives a whole list, but the next one works on one record at a time. Put "Do this for each record" in between.';
	}
	return `This step gives ${portName(from.type)}, but "${to.label}" needs ${portName(to.type)}.`;
}

export function findPort(def: NodeTypeDto | undefined, side: 'in' | 'out', id: string) {
	const ports = side === 'in' ? def?.inputs : def?.outputs;
	return ports?.find((port) => port.id === id);
}

// ---------------------------------------------------------------------------
// Friendly schedule text

export interface ScheduleValue {
	every: 'day' | 'week' | 'month';
	at: string;
	weekdays?: string[];
	day_of_month?: number | null;
	timezone?: string;
}

export const WEEKDAYS = [
	'monday',
	'tuesday',
	'wednesday',
	'thursday',
	'friday',
	'saturday',
	'sunday'
] as const;

function capitalise(text: string): string {
	return text.charAt(0).toUpperCase() + text.slice(1);
}

function ordinal(day: number): string {
	const rest = day % 100;
	if (rest >= 11 && rest <= 13) return `${day}th`;
	switch (day % 10) {
		case 1:
			return `${day}st`;
		case 2:
			return `${day}nd`;
		case 3:
			return `${day}rd`;
		default:
			return `${day}th`;
	}
}

function joinWords(words: string[]): string {
	if (words.length <= 1) return words.join('');
	return `${words.slice(0, -1).join(', ')} and ${words[words.length - 1]}`;
}

export function isScheduleValue(value: unknown): value is ScheduleValue {
	if (typeof value !== 'object' || value === null) return false;
	const candidate = value as Record<string, unknown>;
	return (
		(candidate.every === 'day' || candidate.every === 'week' || candidate.every === 'month') &&
		typeof candidate.at === 'string'
	);
}

export function describeSchedule(value: unknown): string {
	if (!isScheduleValue(value)) return 'On a schedule';
	const zone = value.timezone && value.timezone !== 'UTC' ? ` (${value.timezone})` : '';
	if (value.every === 'day') return `Every day at ${value.at}${zone}`;
	if (value.every === 'week') {
		const days = (value.weekdays?.length ? value.weekdays : ['monday']).map(capitalise);
		return `Every ${joinWords(days)} at ${value.at}${zone}`;
	}
	const day = value.day_of_month ?? 1;
	return `On the ${ordinal(day)} of every month at ${value.at}${zone}`;
}

// ---------------------------------------------------------------------------
// Publication search

export interface PublicationQuery {
	terms: string;
	keywords?: string[];
	authors?: string[];
	journal?: string;
}

function quoted(term: string): string {
	const trimmed = term.trim();
	return /\s/.test(trimmed) ? `"${trimmed}"` : trimmed;
}

/** Turns the simple search form into the words the publication sources understand. */
export function composeQueryTerms(query: Omit<PublicationQuery, 'terms'>): string {
	const parts: string[] = [];
	const keywords = (query.keywords ?? []).map((word) => word.trim()).filter(Boolean);
	if (keywords.length) parts.push(`(${keywords.map(quoted).join(' OR ')})`);
	const authors = (query.authors ?? []).map((name) => name.trim()).filter(Boolean);
	if (authors.length)
		parts.push(`(${authors.map((name) => `${quoted(name)}[Author]`).join(' OR ')})`);
	const journal = query.journal?.trim();
	if (journal) parts.push(`${quoted(journal)}[Journal]`);
	return parts.join(' AND ');
}

export function describeQuery(value: unknown): string {
	if (typeof value !== 'object' || value === null) return '';
	const query = value as PublicationQuery;
	const parts: string[] = [];
	const keywords = (query.keywords ?? []).filter(Boolean);
	if (keywords.length) parts.push(joinWords(keywords.map((word) => `"${word}"`)));
	const authors = (query.authors ?? []).filter(Boolean);
	if (authors.length) parts.push(`by ${joinWords(authors)}`);
	if (query.journal?.trim()) parts.push(`in ${query.journal.trim()}`);
	if (parts.length) return parts.join(' ');
	return query.terms?.trim() ?? '';
}

// ---------------------------------------------------------------------------
// Conditions

export interface ConditionRuleValue {
	field: string;
	operator: string;
	value?: unknown;
}
export interface ConditionValue {
	match: 'all' | 'any';
	rules: ConditionRuleValue[];
}

export const OPERATORS: { value: string; label: string; needsValue: boolean }[] = [
	{ value: 'equals', label: 'is', needsValue: true },
	{ value: 'not_equals', label: 'is not', needsValue: true },
	{ value: 'contains', label: 'contains', needsValue: true },
	{ value: 'not_contains', label: 'does not contain', needsValue: true },
	{ value: 'starts_with', label: 'starts with', needsValue: true },
	{ value: 'ends_with', label: 'ends with', needsValue: true },
	{ value: 'greater_than', label: 'is more than', needsValue: true },
	{ value: 'less_than', label: 'is less than', needsValue: true },
	{ value: 'is_empty', label: 'is empty', needsValue: false },
	{ value: 'is_not_empty', label: 'is filled in', needsValue: false }
];

/** Common pieces of information a rule can look at, in everyday words. */
export const CONDITION_FIELDS: { value: string; label: string }[] = [
	{ value: 'title', label: 'Title' },
	{ value: 'abstract', label: 'Abstract' },
	{ value: 'year', label: 'Year' },
	{ value: 'journal', label: 'Journal' },
	{ value: 'authors', label: 'Authors' },
	{ value: 'doi', label: 'DOI' },
	{ value: 'count', label: 'Number of records' }
];

export function fieldLabel(field: string): string {
	return CONDITION_FIELDS.find((item) => item.value === field)?.label.toLowerCase() ?? field;
}

/** The text a detail is written as in a block's text, for example `{{title}}`. */
export function detailToken(key: string): string {
	return `{{${key}}}`;
}

/**
 * Put a detail's token into a text where the caret or selection is, replacing
 * any selected text. Returns the new text and where the caret should go.
 */
export function insertDetail(
	text: string,
	key: string,
	selectionStart: number | null,
	selectionEnd: number | null
): { text: string; caret: number } {
	const start = Math.min(Math.max(selectionStart ?? text.length, 0), text.length);
	const end = Math.min(Math.max(selectionEnd ?? start, start), text.length);
	const token = detailToken(key);
	return {
		text: text.slice(0, start) + token + text.slice(end),
		caret: start + token.length
	};
}

export function describeCondition(value: unknown): string {
	if (typeof value !== 'object' || value === null) return '';
	const condition = value as ConditionValue;
	const rules = (condition.rules ?? []).filter((rule) => rule.field);
	if (!rules.length) return '';
	const sentences = rules.map((rule) => {
		const operator = OPERATORS.find((item) => item.value === rule.operator);
		const tail = operator?.needsValue ? ` "${String(rule.value ?? '')}"` : '';
		return `${fieldLabel(rule.field)} ${operator?.label ?? rule.operator}${tail}`;
	});
	return sentences.join(condition.match === 'any' ? ' or ' : ' and ');
}

// ---------------------------------------------------------------------------
// Block summaries

function text(value: unknown): string {
	return typeof value === 'string' ? value.trim() : '';
}

function shorten(value: string, max = 70): string {
	const single = value.replace(/\s+/g, ' ').trim();
	return single.length > max ? `${single.slice(0, max - 1)}…` : single;
}

function optionLabel(def: NodeTypeDto, key: string, value: unknown): string {
	const field = def.config.find((item) => item.key === key);
	return field?.options.find((option) => option.value === value)?.label ?? text(value);
}

const DURATION_UNIT: Record<string, string> = {
	seconds: 'second',
	minutes: 'minute',
	hours: 'hour',
	days: 'day'
};

export function describeDuration(value: unknown): string {
	if (typeof value !== 'object' || value === null) return '';
	const { value: amount, unit } = value as { value?: number; unit?: string };
	if (!amount || !unit) return '';
	const base = DURATION_UNIT[unit] ?? unit;
	return `${amount} ${base}${amount === 1 ? '' : 's'}`;
}

export function hostOf(url: string): string {
	try {
		return new URL(url).host;
	} catch {
		return url;
	}
}

/** One plain line describing how a block is set up. */
export function summarizeNode(
	def: NodeTypeDto | undefined,
	config: Record<string, unknown>
): string {
	if (!def) return '';
	switch (def.id) {
		case 'trigger.schedule':
			return describeSchedule(config.schedule);
		case 'trigger.publication_alert': {
			const query = describeQuery(config.query);
			const where = optionLabel(def, 'sources', config.sources ?? 'both');
			const when = describeSchedule(config.schedule).toLowerCase();
			return query
				? `${query} — ${[where, when].filter(Boolean).join(', ')}`
				: 'Choose what to look for';
		}
		case 'logic.if':
		case 'logic.filter':
			return describeCondition(config.condition) || 'Build a rule';
		case 'logic.wait':
			return `Wait ${describeDuration(config.duration) || 'a while'}`;
		case 'logic.for_each':
			return `Up to ${Number(config.max_items ?? 50)} records`;
		case 'data.find_records':
		case 'data.screening_queue': {
			const status = text(config.status);
			const words = text(config.text);
			const bits = [
				status && status !== 'any' ? optionLabel(def, 'status', status) : '',
				words ? `containing "${words}"` : ''
			].filter(Boolean);
			return bits.length ? bits.join(' ') : 'Any record';
		}
		case 'action.record_decision': {
			const stage = optionLabel(def, 'stage', config.stage);
			const decision = optionLabel(def, 'decision', config.decision);
			return decision ? `${decision}${stage ? ` · ${stage.toLowerCase()}` : ''}` : '';
		}
		case 'action.export':
			return `Save as ${optionLabel(def, 'format', config.format ?? 'ris').split(' ')[0]}`;
		case 'ai.prompt':
			return shorten(text(config.instructions));
		case 'ai.classify':
			return optionLabel(def, 'stage', config.stage);
		case 'ai.run_review':
			return optionLabel(def, 'task', config.task);
		case 'integration.http_request': {
			const url = text(config.url);
			return url ? `${text(config.method) || 'POST'} ${hostOf(url)}` : '';
		}
		case 'integration.notify':
			return shorten(text(config.title));
		case 'integration.email': {
			const to = text(config.to);
			return to ? `To ${to}: ${shorten(text(config.subject), 40)}` : '';
		}
		case 'integration.slack':
			return shorten(text(config.message));
		default:
			break;
	}
	return shorten(def.description, 80);
}

// ---------------------------------------------------------------------------
// Trigger text for the list

const EVENT_PHRASE: Record<string, string> = {
	'trigger.report_added': 'When a record is added',
	'trigger.acquisition_completed': 'When an import or search finishes',
	'trigger.full_text_attached': 'When a full text is attached',
	'trigger.report_included': 'When a record is included',
	'trigger.study_created': 'When a study is created',
	'trigger.appraisal_completed': 'When a quality appraisal is completed',
	'trigger.screening_decision_recorded': 'When a screening decision is recorded',
	'trigger.protocol_published': 'When the protocol is published',
	'trigger.document_parsed': 'When a full text finishes processing',
	'trigger.manual': 'When you press Run',
	'trigger.webhook': 'When another tool calls its web address',
	'trigger.email': 'When an e-mail arrives'
};

export function describeTrigger(graph: WfGraph): string {
	const triggers = graph.nodes.filter((node) => node.type.startsWith('trigger.'));
	if (triggers.length === 0) return 'No trigger yet';
	const node = triggers[0];
	const config = (node.config ?? {}) as Record<string, unknown>;
	if (node.type === 'trigger.schedule') return describeSchedule(config.schedule);
	if (node.type === 'trigger.publication_alert') {
		const query = describeQuery(config.query);
		const when = describeSchedule(config.schedule).toLowerCase();
		return query
			? `When new publications match ${query} (checked ${when})`
			: 'When new publications match a search';
	}
	return EVENT_PHRASE[node.type] ?? 'When something happens';
}

// ---------------------------------------------------------------------------
// Sample data for test runs

export function sampleTriggerData(triggerType: string | undefined): Record<string, unknown> {
	const record = {
		id: '00000000-0000-4000-8000-000000000001',
		title: 'Sample paper: effect of exercise on sleep quality',
		year: 2024,
		doi: '10.1000/sample.2024.001',
		journal: 'Journal of Sample Studies',
		authors: ['A. Sample', 'B. Example']
	};
	switch (triggerType) {
		case 'trigger.publication_alert':
			return { records: [record, { ...record, title: 'Another sample paper', year: 2023 }] };
		case 'trigger.email':
			return {
				data: {
					subject: 'Interesting paper',
					from: 'colleague@example.org',
					body: 'Have a look at 10.1000/sample.2024.001'
				},
				text: 'Have a look at 10.1000/sample.2024.001'
			};
		case 'trigger.webhook':
			return { data: { example: 'value' } };
		case 'trigger.acquisition_completed':
		case 'trigger.protocol_published':
		case 'trigger.manual':
		case 'trigger.schedule':
			return { data: { note: 'Sample run' } };
		case 'trigger.full_text_attached':
		case 'trigger.document_parsed':
			return { document: { id: record.id, title: record.title } };
		case 'trigger.study_created':
			return { study: { id: record.id, title: record.title } };
		default:
			return { record };
	}
}

// ---------------------------------------------------------------------------
// Graph <-> canvas

export interface FlowBlockData extends BuilderNodeData {
	typeId: string;
	config: Record<string, unknown>;
	customLabel: string | null;
}

export type FlowNode = Node<FlowBlockData>;

export function catalogIndex(catalog: NodeTypeDto[]): Map<string, NodeTypeDto> {
	return new Map(catalog.map((def) => [def.id, def]));
}

function toBuilderPorts(ports: PortDto[]): BuilderPort[] {
	return ports.map((port) => ({ id: port.id, label: port.label, type: port.type }));
}

export function defaultConfig(def: NodeTypeDto): Record<string, unknown> {
	const config: Record<string, unknown> = {};
	for (const field of def.config) {
		if (field.default !== undefined && field.default !== null)
			config[field.key] = field.default;
	}
	return config;
}

export function blockData(
	def: NodeTypeDto | undefined,
	typeId: string,
	config: Record<string, unknown>,
	customLabel: string | null
): FlowBlockData {
	return {
		typeId,
		config,
		customLabel,
		title: customLabel?.trim() || def?.label || 'Unknown block',
		kind: customLabel?.trim() ? def?.label : undefined,
		summary: summarizeNode(def, config),
		tone: toneOf(def?.category ?? 'action'),
		iconKey: typeId,
		inputs: toBuilderPorts(def?.inputs ?? []),
		outputs: toBuilderPorts(def?.outputs ?? []),
		dryRun: def?.has_side_effects ?? false
	};
}

export function edgeId(edge: WfEdge): string {
	return edge.id || `${edge.from.node}:${edge.from.port}->${edge.to.node}:${edge.to.port}`;
}

export function graphToFlow(
	graph: WfGraph,
	catalog: Map<string, NodeTypeDto>
): { nodes: FlowNode[]; edges: Edge[] } {
	const nodes: FlowNode[] = graph.nodes.map((node) => ({
		id: node.id,
		type: 'block',
		position: { x: node.position.x, y: node.position.y },
		data: blockData(
			catalog.get(node.type),
			node.type,
			(node.config ?? {}) as Record<string, unknown>,
			node.label ?? null
		)
	}));
	const edges: Edge[] = graph.edges.map((edge) => ({
		id: edgeId(edge),
		source: edge.from.node,
		sourceHandle: edge.from.port,
		target: edge.to.node,
		targetHandle: edge.to.port
	}));
	return { nodes, edges };
}

export function flowToGraph(
	nodes: Pick<FlowNode, 'id' | 'type' | 'position' | 'data'>[],
	edges: Pick<Edge, 'id' | 'source' | 'target' | 'sourceHandle' | 'targetHandle'>[]
): WfGraph {
	const blocks = nodes.filter((node) => node.type === 'block');
	const ids = new Set(blocks.map((node) => node.id));
	return {
		nodes: blocks.map((node) => ({
			id: node.id,
			type: node.data.typeId,
			position: { x: Math.round(node.position.x), y: Math.round(node.position.y) },
			...(node.data.customLabel ? { label: node.data.customLabel } : {}),
			config: node.data.config
		})),
		edges: edges
			.filter((edge) => ids.has(edge.source) && ids.has(edge.target))
			.map((edge) => ({
				id: edge.id,
				from: { node: edge.source, port: edge.sourceHandle ?? '' },
				to: { node: edge.target, port: edge.targetHandle ?? '' }
			}))
	};
}

export function newNodeId(typeId: string, existing: Iterable<string>): string {
	const taken = new Set(existing);
	const base = typeId.split('.').pop() ?? 'step';
	for (let index = 1; ; index += 1) {
		const candidate = `${base}_${index}`;
		if (!taken.has(candidate)) return candidate;
	}
}

/** Simple left-to-right layered layout used by "Auto-arrange". */
export function autoArrange<T extends { id: string; position: { x: number; y: number } }>(
	nodes: T[],
	edges: { source: string; target: string }[],
	spacing = { x: 340, y: 190 }
): T[] {
	const depth = new Map<string, number>();
	const incoming = new Map<string, string[]>();
	for (const node of nodes) incoming.set(node.id, []);
	for (const edge of edges) incoming.get(edge.target)?.push(edge.source);
	const visiting = new Set<string>();
	const depthOf = (id: string): number => {
		const known = depth.get(id);
		if (known !== undefined) return known;
		if (visiting.has(id)) return 0;
		visiting.add(id);
		const parents = incoming.get(id) ?? [];
		const value = parents.length ? Math.max(...parents.map(depthOf)) + 1 : 0;
		visiting.delete(id);
		depth.set(id, value);
		return value;
	};
	const rows = new Map<number, number>();
	const ordered = [...nodes].sort(
		(a, b) => a.position.y - b.position.y || a.position.x - b.position.x
	);
	const placed = new Map<string, { x: number; y: number }>();
	for (const node of ordered) {
		const column = depthOf(node.id);
		const row = rows.get(column) ?? 0;
		rows.set(column, row + 1);
		placed.set(node.id, { x: 60 + column * spacing.x, y: 60 + row * spacing.y });
	}
	return nodes.map((node) => ({ ...node, position: placed.get(node.id) ?? node.position }));
}

// ---------------------------------------------------------------------------
// Test-run previews

export interface NodeRunLike {
	node_id: string;
	status: string;
	note?: string | null;
	error?: string | null;
	input?: unknown;
	output?: unknown;
}

function countOf(value: unknown): number | null {
	if (Array.isArray(value)) return value.length;
	if (typeof value === 'object' && value !== null) {
		let best: number | null = null;
		for (const item of Object.values(value)) {
			if (Array.isArray(item)) best = Math.max(best ?? 0, item.length);
		}
		return best;
	}
	return null;
}

function plural(count: number, word: string): string {
	return `${count} ${word}${count === 1 ? '' : 's'}`;
}

/** "12 in → 9 yes / 3 no" style line for a node after a run. */
export function previewForRun(def: NodeTypeDto | undefined, runs: NodeRunLike[]): string {
	if (runs.length === 0) return '';
	if (runs.length > 1) {
		const done = runs.filter((run) => run.status === 'completed').length;
		return `Ran ${plural(runs.length, 'time')}, ${done} done`;
	}
	const run = runs[0];
	if (run.status === 'failed') return run.error ?? 'Something went wrong';
	if (run.status === 'skipped') return 'Nothing reached this step';
	// AI screening writes its counts per verdict (and its progress while it waits) into the note.
	if (run.note && def?.outputs.some((port) => port.id === 'included')) return run.note;
	const input = countOf(run.input);
	if (
		def?.outputs.length &&
		def.outputs.length > 1 &&
		typeof run.output === 'object' &&
		run.output
	) {
		const output = run.output as Record<string, unknown>;
		const parts = def.outputs
			.map((port) => {
				const value = output[port.id];
				if (value === undefined || value === null) return null;
				const count = Array.isArray(value) ? value.length : 1;
				return `${count} ${port.label.toLowerCase()}`;
			})
			.filter((part): part is string => part !== null);
		if (parts.length) return `${input !== null ? `${input} in → ` : ''}${parts.join(' / ')}`;
	}
	const output = countOf(run.output);
	if (input !== null && output !== null) return `${input} in → ${output} out`;
	if (output !== null) return `${plural(output, 'record')} out`;
	return run.note ?? '';
}

export function statusOfRuns(runs: NodeRunLike[]): string | undefined {
	if (runs.length === 0) return undefined;
	for (const status of ['failed', 'running', 'queued', 'cancelled']) {
		if (runs.some((run) => run.status === status)) return status;
	}
	if (runs.every((run) => run.status === 'skipped')) return 'skipped';
	return 'completed';
}

export function isFieldKind(field: ConfigFieldDto, ...kinds: string[]): boolean {
	return kinds.includes(field.kind);
}

/** A name not yet used by another automation, e.g. "Untitled automation 2". */
export function uniqueName(base: string, existing: string[]): string {
	const taken = new Set(existing.map((name) => name.trim().toLowerCase()));
	if (!taken.has(base.trim().toLowerCase())) return base;
	for (let index = 2; ; index += 1) {
		const candidate = `${base} ${index}`;
		if (!taken.has(candidate.toLowerCase())) return candidate;
	}
}
