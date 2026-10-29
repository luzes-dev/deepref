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
		hidden: false,
		get textContent() {
			return `queue-age:pool=documents Document wait ${Object.values(
				nodes,
			)
				.map((node) => node.textContent)
				.join(" ")}`;
		},
		querySelector: (selector) => nodes[selector],
	};
	let onInput;
	const filter = {
		value: "",
		addEventListener: (event, callback) => {
			assert.equal(event, "input");
			onInput = callback;
		},
	};
	const count = { textContent: "All panels shown." };
	const unknown = {
		dataset: {},
		hidden: false,
		textContent: "backup-age Backup age unknown platform-owner",
	};
	const rows = [row, unknown];
	const clock = { now };
	let refresh;
	let requests = 0;
	const context = {
		document: {
			querySelectorAll: (selector) => {
				if (selector === "tr[data-expire-at]") return [row];
				assert.equal(selector, "tbody tr");
				return rows;
			},
			getElementById: (id) => {
				if (id === "panel-filter") return filter;
				assert.equal(id, "panel-count");
				return count;
			},
		},
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
	return {
		nodes,
		row,
		clock,
		refresh,
		filter,
		count,
		visible: () => rows.filter((item) => !item.hidden),
		input: (value) => {
			filter.value = value;
			assert.equal(
				typeof onInput,
				"function",
				"planned local filter is missing",
			);
			onInput();
		},
		requests: () => requests,
	};
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

test("local panel filter narrows literal text and clear restores all panels", () => {
	assert.match(
		html,
		/<label[^>]*for="panel-filter"/,
		"planned labelled local filter is missing",
	);
	assert.match(html, /<input[^>]*maxlength="80"/);
	assert.match(html, /aria-controls="panel-table"/);
	assert.match(html, /id="panel-count"[^>]*role="status"/);
	const state = run();
	assert.equal(state.count.textContent, "Showing 2 of 2 panels");
	state.input("DoCuMenTs");
	assert.deepEqual(state.visible(), [state.row]);
	assert.equal(state.count.textContent, "Showing 1 of 2 panels");
	state.input(".*");
	assert.equal(state.visible().length, 0);
	assert.equal(state.count.textContent, "Showing 0 of 2 panels");
	state.input("");
	assert.equal(state.visible().length, 2);
	assert.equal(state.nodes[".reading"].textContent, "7");
	assert.equal(state.requests(), 0);
});

test("local filter bounds programmatic input without interpreting markup", () => {
	const state = run();
	state.input("x".repeat(80) + "documents");
	assert.equal(state.filter.value.length, 80);
	assert.equal(state.visible().length, 0);
	state.input("<script>documents</script>");
	assert.equal(state.visible().length, 0);
	assert.equal(state.requests(), 0);
});

test("expiry reapplies the current filter and clearing never rejuvenates a row", () => {
	const state = run();
	state.input("unknown");
	assert.equal(state.visible().length, 1);
	state.clock.now = 2001;
	state.refresh();
	assert.equal(state.visible().length, 2);
	assert.equal(state.count.textContent, "Showing 2 of 2 panels");
	state.clock.now = 1500;
	state.input("");
	state.refresh();
	assert.equal(state.nodes[".reading"].textContent, "unknown");
	assert.equal(state.nodes[".status"].textContent, "unknown");
	assert.equal(state.nodes[".proposal"].textContent, "");
	assert.equal(state.requests(), 0);
});
