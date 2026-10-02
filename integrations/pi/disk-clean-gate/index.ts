/**
 * disk-clean gate for pi.
 *
 * pi runs tool calls without asking, so this extension makes the one
 * destructive disk-clean command wait for a person. On `disk-clean apply <id>`
 * it reads the plan file itself and shows what will be deleted: the approval
 * does not depend on how the model described the plan. `disk-clean clean` is
 * blocked outright; it is meant for people at a terminal.
 *
 * When in doubt it blocks: an id it cannot read literally (`"$ID"`), a plan it
 * cannot find, or no UI to ask in.
 */

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { readdirSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { isAbsolute, join } from "node:path";

// `disk-clean` as a command word: at the start, after a separator or quote, or with a path in front.
const WORD = String.raw`(?:^|[\s;&|()\x60'"])(?:[^\s;&|()\x60'"]*/)?disk-clean`;
const APPLY = new RegExp(String.raw`${WORD}\s+apply\b([^;&|\n]*)`, "g");
const CLEAN = new RegExp(String.raw`${WORD}\s+clean\b`);
const PLAN_ID = /^[0-9a-f]{1,32}$/;
const PATHS_PER_ITEM = 8;

interface PlanPath {
	path: string;
	bytes: number;
	unreadable: number;
}

interface PlanItem {
	id: string;
	safety: string;
	notes: string[];
	bytes: number;
	paths: PlanPath[];
}

interface Plan {
	id: string;
	expires_at: number;
	total_bytes: number;
	items: PlanItem[];
	refused: { path: string; reason: string }[];
}

export function plansDir(): string {
	const state = process.env.XDG_STATE_HOME;
	const base = state && isAbsolute(state) ? state : join(homedir(), ".local/state");
	return join(base, "disk-clean/plans");
}

function size(n: number): string {
	const units = ["B", "KiB", "MiB", "GiB", "TiB"];
	let v = n;
	let u = 0;
	while (v >= 1024 && u < units.length - 1) {
		v /= 1024;
		u++;
	}
	return u === 0 ? `${n} B` : `${v >= 100 ? v.toFixed(0) : v.toFixed(1)} ${units[u]}`;
}

function tilde(path: string): string {
	const home = homedir();
	return path.startsWith(`${home}/`) ? `~${path.slice(home.length)}` : path;
}

function clock(secs: number): string {
	const d = new Date(secs * 1000);
	return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

/** The plan ids a command would apply, or the reason it cannot be told. */
export function appliedIds(command: string): { ids: string[] } | { problem: string } {
	const ids: string[] = [];
	for (const match of command.matchAll(APPLY)) {
		const id = match[1]
			.trim()
			.split(/\s+/)
			.find((token) => token !== "" && !token.startsWith("-"));
		if (id === undefined) return { problem: "disk-clean apply 에 계획 ID 가 없습니다." };
		if (!PLAN_ID.test(id)) {
			return { problem: `계획 ID 는 그대로 적어야 확인 창에 보여 줄 수 있습니다: ${id}` };
		}
		ids.push(id);
	}
	return { ids };
}

export function readPlan(id: string): Plan | undefined {
	try {
		return JSON.parse(readFileSync(join(plansDir(), `${id}.json`), "utf8")) as Plan;
	} catch {
		return undefined;
	}
}

export function describe(plan: Plan): string {
	const count = plan.items.reduce((n, i) => n + i.paths.length, 0);
	const lines = [`계획 ${plan.id} · ${size(plan.total_bytes)} · 경로 ${count}개 · ${clock(plan.expires_at)} 까지 유효`, ""];
	for (const item of plan.items) {
		lines.push(`${size(item.bytes).padStart(9)}  ${item.id}  (${item.safety})`);
		for (const note of item.notes) lines.push(`             ${note}`);
		const unreadable = item.paths.reduce((n, p) => n + p.unreadable, 0);
		if (unreadable > 0) lines.push(`             ⚠ 읽지 못한 곳 ${unreadable}곳 — 실제 크기는 이보다 큼`);
		for (const p of item.paths.slice(0, PATHS_PER_ITEM)) lines.push(`             ${tilde(p.path)}`);
		if (item.paths.length > PATHS_PER_ITEM) lines.push(`             … 외 ${item.paths.length - PATHS_PER_ITEM}개`);
	}
	if (plan.refused.length > 0) lines.push("", `건너뜀 ${plan.refused.length}곳 (지우지 않음)`);
	lines.push("", "지운 것은 되돌릴 수 없습니다.");
	return lines.join("\n");
}

export default function (pi: ExtensionAPI) {
	pi.on("tool_call", async (event, ctx) => {
		if (event.toolName !== "bash") return undefined;
		const command = String((event.input as { command?: unknown }).command ?? "");
		if (!command.includes("disk-clean")) return undefined;

		if (CLEAN.test(command)) {
			return {
				block: true,
				reason: "disk-clean clean 은 터미널의 사람용입니다. `disk-clean plan <ID…> --json` 으로 계획을 만들어 보여 주고, 승인을 받은 뒤 `disk-clean apply <계획 ID>` 를 실행하세요.",
			};
		}

		const parsed = appliedIds(command);
		if ("problem" in parsed) return { block: true, reason: parsed.problem };
		if (parsed.ids.length === 0) return undefined;

		const plans: Plan[] = [];
		for (const id of parsed.ids) {
			const plan = readPlan(id);
			if (plan === undefined) {
				return { block: true, reason: `계획 ${id} 가 없습니다(이미 실행했거나 만든 적 없음). 다시 plan 하세요.` };
			}
			plans.push(plan);
		}
		if (!ctx.hasUI) {
			return { block: true, reason: "disk-clean apply 는 사람의 승인이 필요한데, 물어볼 UI 가 없습니다." };
		}
		const ok = await ctx.ui.confirm("disk-clean: 이 계획대로 지울까요?", plans.map(describe).join("\n\n"));
		if (!ok) return { block: true, reason: "사용자가 이 삭제 계획을 승인하지 않았습니다." };
		return undefined;
	});

	pi.registerCommand("disk-clean-plans", {
		description: "실행을 기다리는 disk-clean 계획을 보여 줍니다",
		handler: async (_args, ctx) => {
			let files: string[] = [];
			try {
				files = readdirSync(plansDir()).filter((f) => f.endsWith(".json"));
			} catch {
				// No plans folder yet.
			}
			const now = Date.now() / 1000;
			const plans = files
				.map((f) => readPlan(f.slice(0, -".json".length)))
				.filter((p): p is Plan => p !== undefined && p.expires_at > now);
			ctx.ui.notify(
				plans.length === 0
					? "기다리는 disk-clean 계획이 없습니다."
					: plans.map((p) => `${p.id}  ${size(p.total_bytes)}  ${p.items.map((i) => i.id).join(", ")}  (${clock(p.expires_at)} 까지)`).join("\n"),
				"info",
			);
		},
	});
}
