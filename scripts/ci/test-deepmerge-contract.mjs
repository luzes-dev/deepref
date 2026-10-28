import { readFileSync } from "node:fs";
import {
	PATCH_HASH,
	verifyDigest,
	verifyLockfile,
} from "./check-deepmerge-patch.mjs";
import assert from "node:assert/strict";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import test from "node:test";
const web = createRequire(
	new URL("../../apps/web/package.json", import.meta.url),
);
const plugin = createRequire(web.resolve("@sveltejs/vite-plugin-svelte"));
const dir = dirname(plugin.resolve("deepmerge"));
for (const entry of ["cjs.js", "umd.js"]) {
	const merge = plugin(join(dir, entry));
	for (const shape of [
		"null-target",
		"source-own",
		"target-own",
		"nested-null",
	]) {
		test(`${entry}: ordinary result prototype for ${shape}`, () => {
			const payload = JSON.parse(
				'{"__proto__":{"inheritedConfigFlag":true},"ordinary":7}',
			);
			const target =
				shape === "null-target"
					? Object.create(null)
					: shape === "target-own"
						? payload
						: {};
			const source = shape === "target-own" ? { additional: 9 } : payload;
			const result =
				shape === "nested-null"
					? merge({ section: null }, { section: source }).section
					: merge(target, source);
			assert.equal(Object.getPrototypeOf(result), Object.prototype);
			assert.equal(result.inheritedConfigFlag, undefined);
			assert.equal(Object.hasOwn(result, "__proto__"), false);
			assert.equal(result.ordinary, 7);
		});
	}
	test(`${entry}: Svelte config merge compatibility`, () => {
		const symbol = Symbol("config");
		const preprocessor = () => "preserved";
		const regex = /component/;
		const defaults = {
			compilerOptions: { dev: false, css: "external" },
			preprocess: [() => "old"],
			regex,
			[symbol]: 1,
		};
		const source = {
			compilerOptions: { dev: true },
			preprocess: [preprocessor],
		};
		const result = merge(defaults, source, {
			arrayMerge: (_target, replacement) => replacement,
		});
		assert.deepEqual(result.compilerOptions, {
			dev: true,
			css: "external",
		});
		assert.deepEqual(result.preprocess, [preprocessor]);
		assert.equal(result.regex, regex);
		assert.equal(result[symbol], 1);
		assert.equal(defaults.compilerOptions.dev, false);
		assert.deepEqual(merge.all([{ values: [1] }, { values: [2] }]), {
			values: [1, 2],
		});
	});
}

const lockfile = readFileSync(
	new URL("../../pnpm-lock.yaml", import.meta.url),
	"utf8",
);
test("reviewed lockfile is accepted", () => verifyLockfile(lockfile));
for (const [name, transform] of [
	[
		"missing patch binding",
		(value) => value.replace("  deepmerge@4.3.1: " + PATCH_HASH, ""),
	],
	[
		"changed patch hash",
		(value) => value.replace(PATCH_HASH, "0".repeat(64)),
	],
	[
		"unpatched consumer",
		(value) =>
			value.replace(
				"      deepmerge: 4.3.1(patch_hash=" + PATCH_HASH + ")",
				"      deepmerge: 4.3.1",
			),
	],
	[
		"extra unpatched snapshot",
		(value) =>
			value.replace(
				"snapshots:\n",
				"snapshots:\n  deepmerge@4.3.1: {}\n",
			),
	],
	[
		"different version",
		(value) => value.replace("  deepmerge@4.3.1:", "  deepmerge@4.2.0:"),
	],
]) {
	test("reject " + name, () =>
		assert.throws(() => verifyLockfile(transform(lockfile))),
	);
}
test("reject changed installed bytes", () =>
	assert.throws(() => verifyDigest(Buffer.from("unpatched"), PATCH_HASH)));
