/**
 * Plain-language copy for notifications. Rows carry internal wording such as
 * "review execution failed: AI output failed semantic validation" and HTML
 * entities from source metadata ("Lancet Diabetes &amp; Endocrinology"). This
 * module turns both into text a researcher can act on and keeps the original
 * wording for a "technical details" disclosure. It also maps each row to the
 * page where its object lives.
 */

export interface PlainText {
	text: string;
	/** The original wording when it was translated; null when shown unchanged. */
	technical: string | null;
}

export interface PlainNotificationText {
	title: string;
	body: string | null;
	technical: string | null;
}

const NAMED_ENTITIES: Readonly<Record<string, string>> = {
	amp: '&',
	lt: '<',
	gt: '>',
	quot: '"',
	apos: "'",
	nbsp: ' '
};

const ENTITY_PATTERN = /&(#[xX][0-9a-fA-F]+|#[0-9]+|[a-zA-Z][a-zA-Z0-9]*);/g;

function fromCodePoint(codePoint: number, original: string): string {
	if (!Number.isInteger(codePoint) || codePoint <= 0 || codePoint > 0x10ffff) return original;
	return String.fromCodePoint(codePoint);
}

/**
 * Decode named and numeric HTML entities in one pass, so "&amp;amp;" becomes
 * "&amp;" and never a second time. Unknown entities are left as written.
 */
export function decodeHtmlEntities(text: string): string {
	return text.replace(ENTITY_PATTERN, (original: string, entity: string) => {
		if (entity.startsWith('#x') || entity.startsWith('#X')) {
			return fromCodePoint(Number.parseInt(entity.slice(2), 16), original);
		}
		if (entity.startsWith('#')) {
			return fromCodePoint(Number.parseInt(entity.slice(1), 10), original);
		}
		return NAMED_ENTITIES[entity] ?? original;
	});
}

/** Known internal failure wording, in the order it is checked. */
const TRANSLATIONS: ReadonlyArray<{ match: RegExp; plain: string }> = [
	{
		match: /insufficient balance/i,
		plain: 'The AI provider account has run out of credit. Top it up, then try again; nothing was lost.'
	},
	{
		match: /subscription limit reached/i,
		plain: 'The AI subscription has reached its usage limit for now. Try again later; nothing was lost.'
	},
	{
		match: /semantic validation|AI output rejected/i,
		plain: 'The AI could not produce a valid suggestion (it did not follow the protocol). Try again or decide yourself.'
	},
	{
		match: /proposal persistence or validation failed/i,
		plain: "The AI's suggestion could not be saved. Try again."
	},
	{
		match: /persistence failed/i,
		plain: "The AI's answer could not be saved. Try again."
	},
	{
		match: /^review execution failed|^The AI review failed/i,
		plain: 'The AI review did not finish. Try again or decide yourself.'
	},
	{
		match: /automation step is not an accepted built-in/i,
		plain: 'This step is not available in this version of DeepRef.'
	}
];

/** Translate one message: known internal wording becomes plain language. */
export function plainText(raw: string): PlainText {
	const decoded = decodeHtmlEntities(raw).trim();
	const rule = TRANSLATIONS.find((entry) => entry.match.test(decoded));
	if (!rule) return { text: decoded, technical: null };
	return { text: rule.plain, technical: decoded };
}

/** Title and body of a notification in plain language, with any original wording. */
export function plainNotificationText(notification: {
	title: string;
	body?: string | null;
}): PlainNotificationText {
	const title = plainText(notification.title);
	const body = notification.body?.trim() ? plainText(notification.body) : null;
	const technical = [title.technical, body?.technical].filter(
		(part): part is string => typeof part === 'string' && part !== ''
	);
	return {
		title: title.text,
		body: body?.text ?? null,
		technical: technical.length > 0 ? technical.join('\n') : null
	};
}

function stringField(payload: unknown, key: string): string | undefined {
	if (typeof payload !== 'object' || payload === null) return undefined;
	const value = (payload as Record<string, unknown>)[key];
	return typeof value === 'string' && value !== '' ? value : undefined;
}

/**
 * The page where a notification's object lives, or null when the row has no
 * project or no known destination. Runs open the automation builder at that
 * run; imports open the import with its inspector.
 */
export function notificationHref(notification: {
	kind: string;
	project_id?: string | null;
	payload?: unknown;
}): string | null {
	const projectId = notification.project_id;
	if (!projectId) return null;
	const base = `/projects/${encodeURIComponent(projectId)}`;
	const workflowId = stringField(notification.payload, 'workflow_id');
	const runId = stringField(notification.payload, 'run_id');
	const acquisitionId = stringField(notification.payload, 'acquisition_id');
	switch (notification.kind) {
		case 'workflow':
		case 'workflow_run.failed': {
			if (!workflowId) return `${base}/automations`;
			const run = runId ? `?run=${encodeURIComponent(runId)}` : '';
			return `${base}/automations/${encodeURIComponent(workflowId)}${run}`;
		}
		case 'acquisition.completed':
		case 'acquisition.failed':
			return acquisitionId
				? `${base}/discovery/imports?ingestion=${encodeURIComponent(acquisitionId)}`
				: `${base}/discovery/imports`;
		case 'review_run.completed':
			return `${base}/recommendations`;
		case 'review_run.failed':
		case 'review_run.blocked':
		case 'automation_run.failed':
		case 'automation_run.completed':
			return `${base}/activity`;
		default:
			return null;
	}
}
