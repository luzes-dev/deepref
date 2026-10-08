import { describe, expect, it } from 'vitest';
import { parseAssistantMarkdown, safeLink, type MarkdownNode } from './markdown';

const ALLOWED_TAGS = new Set([
	'p',
	'h1',
	'h2',
	'h3',
	'h4',
	'h5',
	'h6',
	'strong',
	'em',
	'del',
	'code',
	'pre',
	'a',
	'ul',
	'ol',
	'li',
	'blockquote',
	'hr',
	'table',
	'thead',
	'tbody',
	'tr',
	'th',
	'td'
]);

/** Every element in the tree, depth first. */
function elements(nodes: MarkdownNode[]): Extract<MarkdownNode, { kind: 'element' }>[] {
	return nodes.flatMap((node) =>
		node.kind === 'element' ? [node, ...elements(node.children)] : []
	);
}

/** All text content, so assertions can check what a reader would see. */
function visibleText(nodes: MarkdownNode[]): string {
	return nodes
		.map((node) => {
			if (node.kind === 'text') return node.text;
			if (node.kind === 'break') return '\n';
			return visibleText(node.children);
		})
		.join('');
}

describe('parseAssistantMarkdown sanitising', () => {
	it('keeps a script tag and an onerror payload as text, never as elements', () => {
		const payload =
			'Result: <script>alert("x")</script> <img src="x" onerror="alert(1)"> <iframe src="https://evil.example"></iframe>';
		const tree = parseAssistantMarkdown(payload);
		const tags = elements(tree).map((element) => element.tag);
		expect(tags.every((tag) => ALLOWED_TAGS.has(tag))).toBe(true);
		expect(tags).not.toContain('script');
		expect(tags).not.toContain('img');
		expect(tags).not.toContain('iframe');
		expect(visibleText(tree)).toContain('<script>alert("x")</script>');
	});

	it('never produces an element for a script that starts a block', () => {
		const tree = parseAssistantMarkdown('<script>document.cookie</script>\n\n# Title');
		expect(elements(tree).map((element) => element.tag)).toEqual(['p', 'h1']);
	});

	it('drops javascript, data and vbscript links but keeps the words', () => {
		const tree = parseAssistantMarkdown(
			'[click](javascript:alert(1)) [data](data:text/html,<script>x</script>) [vb](vbscript:msgbox)'
		);
		expect(elements(tree).filter((element) => element.tag === 'a')).toEqual([]);
		// markdown-it does not build these links at all, so the source stays as text.
		expect(visibleText(tree)).toContain('[click](javascript:alert(1))');
	});

	it('renders code spans and fences with their source text untouched', () => {
		const tree = parseAssistantMarkdown(
			'Use `<img onerror=x>` here.\n\n```\n<script>1</script>\n```'
		);
		const code = elements(tree).filter((element) => element.tag === 'code');
		expect(code.map((element) => element.block === true)).toEqual([false, true]);
		expect(visibleText(code)).toContain('<img onerror=x>');
		expect(visibleText(code)).toContain('<script>1</script>');
	});

	it('keeps the words of a link whose target is rejected', () => {
		const tree = parseAssistantMarkdown('[see](//evil.example/x)');
		expect(elements(tree).some((element) => element.tag === 'a')).toBe(false);
		expect(visibleText(tree)).toBe('see');
	});
});

describe('safeLink', () => {
	it('accepts same-app paths, http(s) and mailto', () => {
		expect(safeLink('/projects/p1/articles?report=r1')).toEqual({
			href: '/projects/p1/articles?report=r1',
			external: false
		});
		expect(safeLink('https://doi.org/10.1000/xyz')).toEqual({
			href: 'https://doi.org/10.1000/xyz',
			external: true
		});
		expect(safeLink('HTTP://Example.org')).toEqual({
			href: 'http://example.org/',
			external: true
		});
		expect(safeLink('mailto:team@example.org')).toEqual({
			href: 'mailto:team@example.org',
			external: false
		});
	});

	it('rejects scripts, data URLs and protocol-relative or backslash tricks', () => {
		for (const raw of [
			'javascript:alert(1)',
			'JaVaScRiPt:alert(1)',
			'java\tscript:alert(1)',
			'data:text/html,hi',
			'vbscript:msgbox',
			'//evil.example',
			'/\\evil.example',
			'/\t/evil.example',
			'file:///etc/passwd',
			'',
			null
		]) {
			expect(safeLink(raw), raw ?? 'null').toBeNull();
		}
	});
});

describe('parseAssistantMarkdown syntax', () => {
	it('parses bold text inside a paragraph', () => {
		expect(parseAssistantMarkdown('**50 records** left')).toEqual([
			{
				kind: 'element',
				tag: 'p',
				children: [
					{
						kind: 'element',
						tag: 'strong',
						children: [{ kind: 'text', text: '50 records' }]
					},
					{ kind: 'text', text: ' left' }
				]
			}
		]);
	});

	it('parses bullet and numbered lists, keeping the start number', () => {
		const tree = parseAssistantMarkdown(
			'- **4** included\n- 5 excluded\n\n3. third\n4. fourth'
		);
		const lists = tree.filter((node) => node.kind === 'element');
		expect(lists.map((list) => list.kind === 'element' && list.tag)).toEqual(['ul', 'ol']);
		const ordered = lists[1];
		expect(ordered?.kind === 'element' && ordered.start).toBe(3);
		expect(elements(tree).filter((element) => element.tag === 'li')).toHaveLength(4);
	});

	it('parses headings, emphasis and strikethrough', () => {
		const tree = parseAssistantMarkdown('## Included\n\n*maybe* ~~no~~');
		expect(elements(tree).map((element) => element.tag)).toEqual(['h2', 'p', 'em', 'del']);
	});

	it('parses tables into header and body cells', () => {
		const tree = parseAssistantMarkdown('| Study | n |\n|---|---|\n| Fitbit | 60 |');
		const tags = elements(tree).map((element) => element.tag);
		expect(tags).toEqual(['table', 'thead', 'tr', 'th', 'th', 'tbody', 'tr', 'td', 'td']);
		expect(visibleText(tree)).toContain('Fitbit');
	});

	it('marks links to the web as external and keeps the in-app ones internal', () => {
		const tree = parseAssistantMarkdown(
			'[paper](https://example.org/p) and [record](/projects/p1/articles?report=r1)'
		);
		const links = elements(tree).filter((element) => element.tag === 'a');
		expect(links.map((link) => [link.href, link.external])).toEqual([
			['https://example.org/p', true],
			['/projects/p1/articles?report=r1', false]
		]);
	});

	it('returns nothing for a blank answer', () => {
		expect(parseAssistantMarkdown('   \n  ')).toEqual([]);
	});
});
