// Pure generated-script behavior with a tiny DOM contract, not browser/import proof.
const assert = require("node:assert/strict");
const fs = require("node:fs");
const test = require("node:test");
const vm = require("node:vm");

const html = fs.readFileSync(process.env.DEEPREF_DASHBOARD_HTML, "utf8");
const scripts = [...html.matchAll(/<script>([\s\S]*?)<\/script>/g)];
assert.equal(scripts.length, 1);
let code = scripts[0][1];
if (process.env.DEEPREF_EXPIRY_COUNTERFACTUAL === "1") {
	assert.equal(code.split("now>expiry||").length, 2);
	code = code.replace("now>expiry||", "");
}

function run(expiry = "2000", capture = "1000", now = 1500) {
	const nodes = {
		".reading": { textContent: "7" },
		".status": { textContent: "observed" },
		".proposal": { textContent: "Unapproved ticket proposal" },
	};
	const row = {
		dataset: { expireAt: expiry, capturedAt: capture },
		querySelector: (selector) => nodes[selector],
	};
	const clock = { now };
	let refresh;
	let requests = 0;
	const context = {
		document: { querySelectorAll: () => [row] },
		Date: { now: () => clock.now },
		setInterval: (callback, interval) => {
			assert.equal(interval, 1000);
			refresh = callback;
		},
		fetch: () => {
			requests += 1;
			throw new Error("unexpected network request");
		},
	};
	vm.runInNewContext(code, context, { timeout: 100 });
	return { nodes, row, clock, refresh, requests: () => requests };
}

test("fresh capture stays observed without a background request", () => {
	const state = run();
	assert.equal(state.nodes[".status"].textContent, "observed");
	assert.equal(state.nodes[".reading"].textContent, "7");
	assert.equal(state.requests(), 0);
});

test("stale capture becomes unknown and clock rollback cannot rejuvenate it", () => {
	const state = run();
	state.clock.now = 2001;
	state.refresh();
	assert.equal(state.nodes[".status"].textContent, "unknown");
	assert.equal(state.nodes[".reading"].textContent, "unknown");
	assert.equal(state.nodes[".proposal"].textContent, "");
	state.clock.now = 1500;
	state.refresh();
	assert.equal(state.nodes[".status"].textContent, "unknown");
	assert.equal(state.requests(), 0);
});

test("future source capture fails closed", () => {
	const state = run("2000", "1600", 1500);
	assert.equal(state.nodes[".status"].textContent, "unknown");
	assert.equal(state.requests(), 0);
});

test("nonfinite expiry fails closed", () => {
	const state = run("invalid");
	assert.equal(state.nodes[".status"].textContent, "unknown");
	assert.equal(state.nodes[".proposal"].textContent, "");
});
