import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join, resolve } from "node:path";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";

export const PATCH_HASH =
	"658bcc27776e130ae51c0ad9597d4a3a52ac867af0fc014b78714ff162845fe0";
const VERSION = "4.3.1";
const PATCH = "patches/deepmerge@4.3.1.patch";
const root = fileURLToPath(new URL("../../", import.meta.url));

function section(text, name) {
	const lines = text.split("\n");
	const start = lines.indexOf(name + ":");
	assert.notEqual(start, -1, "missing lockfile section: " + name);
	const end = lines.findIndex(
		(line, index) => index > start && /^\S/.test(line),
	);
	return lines.slice(start + 1, end === -1 ? undefined : end).join("\n");
}

function packageKeys(text) {
	return Array.from(
		text.matchAll(/^  ['"]?(deepmerge@[^'"\n]+?)['"]?:/gm),
		(match) => match[1],
	);
}

export function verifyLockfile(text) {
	assert.deepEqual(packageKeys(section(text, "packages")), [
		"deepmerge@" + VERSION,
	]);
	const snapshot = "deepmerge@" + VERSION + "(patch_hash=" + PATCH_HASH + ")";
	assert.deepEqual(packageKeys(section(text, "snapshots")), [snapshot]);
	assert.match(
		section(text, "patchedDependencies"),
		new RegExp("^  deepmerge@4\\.3\\.1: " + PATCH_HASH + "$", "m"),
	);
	const edges = Array.from(
		text.matchAll(/^      deepmerge: (.+)$/gm),
		(match) => match[1],
	);
	assert.ok(edges.length > 0, "expected the Svelte plugin dependency");
	assert.ok(
		edges.every(
			(edge) => edge === VERSION + "(patch_hash=" + PATCH_HASH + ")",
		),
		"unpatched dependency edge",
	);
}

export function verifyDigest(bytes, expected) {
	assert.equal(
		createHash("sha256").update(bytes).digest("hex"),
		expected,
		"patched source identity changed",
	);
}

function command(command, args) {
	const result = spawnSync(command, args, {
		cwd: root,
		encoding: "utf8",
		timeout: 30_000,
	});
	assert.equal(result.error, undefined, "dependency identity command failed");
	assert.equal(result.status, 0, "dependency identity command failed");
	return result.stdout;
}

export function checkInstalledPatch() {
	verifyDigest(readFileSync(join(root, PATCH)), PATCH_HASH);
	const configured = JSON.parse(
		command("pnpm", ["config", "get", "patchedDependencies", "--json"]),
	);
	assert.equal(
		resolve(configured["deepmerge@" + VERSION]),
		join(root, PATCH),
	);
	verifyLockfile(readFileSync(join(root, "pnpm-lock.yaml"), "utf8"));

	const web = createRequire(join(root, "apps/web/package.json"));
	const plugin = createRequire(web.resolve("@sveltejs/vite-plugin-svelte"));
	const directory = dirname(plugin.resolve("deepmerge"));
	assert.equal(
		JSON.parse(readFileSync(join(directory, "../package.json"), "utf8"))
			.version,
		VERSION,
	);
	const files = {
		"../index.js":
			"4eca846bbb57dee82c76977159300725393dbcab1b16ba80c5e685a515391477",
		"cjs.js":
			"d83363b138298a3133d77333ac13f5e058081cb59ac51d78e32f200375576222",
		"umd.js":
			"ac852336902230e07ebe122749ec3c62dfae7915939021004771b89d2e2347ca",
	};
	for (const [name, hash] of Object.entries(files))
		verifyDigest(readFileSync(join(directory, name)), hash);
	return {
		advisory: "GHSA-gcqx-w48m-w2h4",
		package: "deepmerge",
		version: VERSION,
		patch_sha256: PATCH_HASH,
		expires: "2027-01-26",
	};
}

if (
	process.argv[1] &&
	resolve(process.argv[1]) === fileURLToPath(import.meta.url)
) {
	try {
		console.log(JSON.stringify(checkInstalledPatch()));
	} catch (error) {
		console.error("Patched deepmerge verification failed:", error.message);
		process.exitCode = 1;
	}
}
