import MarkdownIt from 'markdown-it';
import type Token from 'markdown-it/lib/token.mjs';

/**
 * Assistant answers, parsed into a small tree of whitelisted elements.
 *
 * markdown-it runs with raw HTML turned off, so `<script>` and event-handler
 * attributes arrive as plain text. The tree is then rendered by Svelte, not
 * through `{@html}`: every piece of text is a text node, and an element can only
 * be one of the tags in `MarkdownTag`. Nothing in an answer can become markup.
 */

export type MarkdownTag =
	| 'p'
	| 'h1'
	| 'h2'
	| 'h3'
	| 'h4'
	| 'h5'
	| 'h6'
	| 'strong'
	| 'em'
	| 'del'
	| 'code'
	| 'pre'
	| 'a'
	| 'ul'
	| 'ol'
	| 'li'
	| 'blockquote'
	| 'hr'
	| 'table'
	| 'thead'
	| 'tbody'
	| 'tr'
	| 'th'
	| 'td';

export type MarkdownNode =
	| { kind: 'text'; text: string }
	| { kind: 'break' }
	| {
			kind: 'element';
			tag: MarkdownTag;
			children: MarkdownNode[];
			/** Set on the `code` inside a `pre`, so it is styled as a block. */
			block?: boolean;
			href?: string;
			external?: boolean;
			start?: number;
	  };

export interface SafeLink {
	href: string;
	/** Opens in a new tab: web addresses outside the app. */
	external: boolean;
}

/** A base that no real link can share, used to tell same-app paths apart. */
const INTERNAL_BASE = 'https://internal.invalid';
const SCHEME = /^[a-z][a-z\d+.-]*:/i;

const markdown = new MarkdownIt({ html: false, linkify: false, typographer: false });

const TAG_BY_TOKEN: Record<string, MarkdownTag> = {
	paragraph_open: 'p',
	bullet_list_open: 'ul',
	ordered_list_open: 'ol',
	list_item_open: 'li',
	blockquote_open: 'blockquote',
	table_open: 'table',
	thead_open: 'thead',
	tbody_open: 'tbody',
	tr_open: 'tr',
	th_open: 'th',
	td_open: 'td',
	strong_open: 'strong',
	em_open: 'em',
	s_open: 'del'
};

const HEADING_TAGS: ReadonlySet<string> = new Set(['h1', 'h2', 'h3', 'h4', 'h5', 'h6']);

/**
 * Accepts only links the chat may show: same-app paths, http(s) and mailto.
 * Resolution follows the browser (tabs and backslashes included), so an input
 * such as `/\evil.example` is rejected rather than trusted as a path.
 */
export function safeLink(raw: string | null | undefined): SafeLink | null {
	const value = (raw ?? '').trim();
	if (!value) return null;
	let url: URL;
	try {
		url = new URL(value, INTERNAL_BASE);
	} catch {
		return null;
	}
	if (!SCHEME.test(value)) {
		return url.origin === INTERNAL_BASE
			? { href: `${url.pathname}${url.search}${url.hash}`, external: false }
			: null;
	}
	if (url.protocol === 'mailto:') return { href: url.href, external: false };
	if (url.protocol === 'http:' || url.protocol === 'https:') {
		return { href: url.href, external: true };
	}
	return null;
}

interface Frame {
	/** `null` unwraps the frame: its children are kept, the wrapper is not. */
	tag: MarkdownTag | null;
	href?: string;
	external?: boolean;
	start?: number;
	children: MarkdownNode[];
}

function frameFor(token: Token): Frame {
	if (token.type === 'link_open') {
		const link = safeLink(token.attrGet('href'));
		return link
			? { tag: 'a', href: link.href, external: link.external, children: [] }
			: { tag: null, children: [] };
	}
	if (token.type === 'ordered_list_open') {
		const start = Number(token.attrGet('start'));
		return {
			tag: 'ol',
			start: Number.isInteger(start) && start > 1 ? start : undefined,
			children: []
		};
	}
	if (token.type === 'heading_open' && HEADING_TAGS.has(token.tag)) {
		return { tag: token.tag as MarkdownTag, children: [] };
	}
	return { tag: TAG_BY_TOKEN[token.type] ?? null, children: [] };
}

function finish(frame: Frame): MarkdownNode[] {
	if (frame.tag === null) return frame.children;
	return [
		{
			kind: 'element',
			tag: frame.tag,
			children: frame.children,
			...(frame.href === undefined
				? {}
				: { href: frame.href, external: frame.external === true }),
			...(frame.start === undefined ? {} : { start: frame.start })
		}
	];
}

function text(value: string): MarkdownNode {
	return { kind: 'text', text: value };
}

/** Leaf tokens: text, breaks, code, rules. Anything unknown is shown as text. */
function leaves(token: Token): MarkdownNode[] {
	switch (token.type) {
		case 'softbreak':
			return [text(' ')];
		case 'hardbreak':
			return [{ kind: 'break' }];
		case 'code_inline':
			return [{ kind: 'element', tag: 'code', children: [text(token.content)] }];
		case 'fence':
		case 'code_block':
			return [
				{
					kind: 'element',
					tag: 'pre',
					children: [
						{
							kind: 'element',
							tag: 'code',
							block: true,
							children: [text(token.content.replace(/\n$/, ''))]
						}
					]
				}
			];
		case 'hr':
			return [{ kind: 'element', tag: 'hr', children: [] }];
		case 'image':
		case 'html_block':
		case 'html_inline':
		default:
			// Images are never loaded from an answer; their alt text and any
			// HTML source are shown as plain text.
			return token.content ? [text(token.content)] : [];
	}
}

function convert(tokens: Token[]): MarkdownNode[] {
	const root: Frame = { tag: null, children: [] };
	const stack: Frame[] = [root];
	const current = (): Frame => stack.at(-1) ?? root;
	const close = (): void => {
		if (stack.length <= 1) return;
		const frame = stack.pop();
		if (frame) current().children.push(...finish(frame));
	};

	for (const token of tokens) {
		if (token.type === 'inline') {
			current().children.push(...convert(token.children ?? []));
		} else if (token.nesting === 1) {
			stack.push(frameFor(token));
		} else if (token.nesting === -1) {
			close();
		} else {
			current().children.push(...leaves(token));
		}
	}
	while (stack.length > 1) close();
	return root.children;
}

/** Parses an assistant answer into a tree that is safe to render as text. */
export function parseAssistantMarkdown(source: string): MarkdownNode[] {
	if (!source.trim()) return [];
	return convert(markdown.parse(source, {}));
}
