/**
 * Reactive stand-ins for `$app/state` and `$app/navigation` in component tests.
 *
 * `goto` resolves on a later task, like a real client-side navigation, and the
 * URL changes only when it settles. `holdNavigation` keeps it pending so a test
 * can act inside the window where the page still shows the old URL.
 */
import { SvelteURL } from 'svelte/reactivity';

export const page = $state<{ url: URL; state: App.PageState }>({
	url: new SvelteURL('http://localhost/'),
	state: {}
});

export const navigations: string[] = [];

let gate: Promise<void> | null = null;

export function resetNavigation(href: string) {
	page.url = new SvelteURL(href, 'http://localhost/');
	page.state = {};
	navigations.length = 0;
	gate = null;
}

export function holdNavigation(): () => void {
	let release = () => {};
	const held = new Promise<void>((resolve) => {
		release = resolve;
	});
	gate = held;
	return () => {
		if (gate === held) gate = null;
		release();
	};
}

export async function goto(url: string | URL, options: { state?: App.PageState } = {}) {
	const target = new SvelteURL(String(url), page.url);
	navigations.push(`${target.pathname}${target.search}`);
	await new Promise((resolve) => setTimeout(resolve, 0));
	if (gate) await gate;
	page.url = target;
	page.state = options.state ?? {};
}

export function resolve(route: string, params: Record<string, string> = {}) {
	return route.replace(/\[(\w+)\]/g, (_, name: string) => params[name] ?? '');
}

/** jsdom has no `matchMedia`; report every query as unmatched (the narrow layout). */
export function stubMatchMedia() {
	window.matchMedia = (query: string) =>
		({
			matches: false,
			media: query,
			onchange: null,
			addEventListener: () => {},
			removeEventListener: () => {},
			addListener: () => {},
			removeListener: () => {},
			dispatchEvent: () => false
		}) as MediaQueryList;
}
