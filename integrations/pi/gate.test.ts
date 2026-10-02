// Run with: node --test integrations/pi/gate.test.ts
import assert from "node:assert/strict";
import { mkdirSync, mkdtempSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { test } from "node:test";

process.env.XDG_STATE_HOME = mkdtempSync(join(tmpdir(), "disk-clean-gate-"));
const gate = await import("./disk-clean-gate/index.ts");

type Handler = (event: unknown, ctx: unknown) => Promise<{ block: boolean; reason: string } | undefined>;

function load() {
	let handler: Handler | undefined;
	const commands: string[] = [];
	gate.default({
		on: (name: string, h: Handler) => {
			if (name === "tool_call") handler = h;
		},
		registerCommand: (name: string) => commands.push(name),
	} as never);
	return { handler: handler!, commands };
}

function plan(id: string, bytes: number) {
	mkdirSync(gate.plansDir(), { recursive: true });
	writeFileSync(
		join(gate.plansDir(), `${id}.json`),
		JSON.stringify({
			id,
			expires_at: Date.now() / 1000 + 3600,
			total_bytes: bytes,
			items: [{ id: "caches/Homebrew", safety: "safe", notes: [], bytes, paths: [{ path: "/Users/x/Library/Caches/Homebrew", bytes, unreadable: 0 }] }],
			refused: [],
		}),
	);
}

function ctx(answer: boolean, hasUI = true) {
	const asked: string[] = [];
	return {
		asked,
		hasUI,
		ui: {
			confirm: async (_title: string, message: string) => {
				asked.push(message);
				return answer;
			},
		},
	};
}

const bash = (command: string) => ({ toolName: "bash", input: { command } });

test("leaves everything else alone", async () => {
	const { handler, commands } = load();
	assert.deepEqual(commands, ["disk-clean-plans"]);
	for (const command of ["ls -la", "disk-clean scan --json", "disk-clean plan caches --json", "disk-clean-foo apply 1"]) {
		assert.equal(await handler(bash(command), ctx(false)), undefined, command);
	}
	assert.equal(await handler({ toolName: "read", input: { path: "x" } }, ctx(false)), undefined);
});

test("blocks clean", async () => {
	const { handler } = load();
	for (const command of ["disk-clean clean caches", "cd /tmp && ~/.cargo/bin/disk-clean clean --dry-run"]) {
		assert.equal((await handler(bash(command), ctx(true)))?.block, true, command);
	}
});

test("asks before apply and shows the plan itself", async () => {
	const { handler } = load();
	plan("a1b2c3", 2 * 1024 ** 3);
	const yes = ctx(true);
	assert.equal(await handler(bash("disk-clean apply a1b2c3 --json"), yes), undefined);
	assert.match(yes.asked[0], /caches\/Homebrew/);
	assert.match(yes.asked[0], /2\.0 GiB/);

	const no = ctx(false);
	assert.equal((await handler(bash("/Users/x/.cargo/bin/disk-clean apply a1b2c3"), no))?.block, true);
	assert.equal(no.asked.length, 1, "the full-path form is asked about too");
});

test("blocks when it cannot show what would be deleted", async () => {
	const { handler } = load();
	plan("a1b2c3", 1);
	const cases = [
		["disk-clean apply", "no id"],
		['disk-clean apply "$ID"', "id not written literally"],
		["disk-clean apply ffff00", "no such plan"],
	];
	for (const [command, why] of cases) {
		const c = ctx(true);
		assert.equal((await handler(bash(command), c))?.block, true, why);
		assert.equal(c.asked.length, 0, why);
	}
	assert.equal((await handler(bash("disk-clean apply a1b2c3"), ctx(true, false)))?.block, true, "no UI");
});

test("one question covers every apply in a command", async () => {
	const { handler } = load();
	plan("aaaa11", 1);
	plan("bbbb22", 2);
	const c = ctx(true);
	assert.equal(await handler(bash("disk-clean apply aaaa11 && echo ok; disk-clean apply bbbb22"), c), undefined);
	assert.equal(c.asked.length, 1);
	assert.match(c.asked[0], /aaaa11[\s\S]*bbbb22/);
});
