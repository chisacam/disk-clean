#!/usr/bin/env python3
"""End-to-end check against a fake home: scan, plan, apply, log, and the refusals.

    cargo build --release && python3 tests/e2e.py target/release/disk-clean

The fake home lives under target/, not the system temp folder: on macOS that
folder is inside the per-user /var/folders directory, where disk-clean only
deletes an app's own cache folders.
"""

import json
import os
import platform
import shutil
import subprocess
import sys
from pathlib import Path

BIN = os.path.abspath(sys.argv[1])
MAC = platform.system() == "Darwin"
HOME = Path(os.path.realpath("target")) / f"e2e-home-{os.getpid()}"
failures = []


def check(cond, what):
    print(("ok    " if cond else "FAIL  ") + what)
    if not cond:
        failures.append(what)


def write(rel, mib=0, text=None):
    path = HOME / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    if text is not None:
        path.write_text(text)
    else:
        path.write_bytes(os.urandom(mib << 20))


def run(*args):
    drop = ("XDG_STATE_HOME", "XDG_CONFIG_HOME", "CLAUDECODE", "PI_SESSION_ID")
    env = {k: v for k, v in os.environ.items() if k not in drop}
    env["HOME"] = str(HOME)
    return subprocess.run([BIN, *args], env=env, capture_output=True, text=True, stdin=subprocess.DEVNULL)


def as_json(proc):
    try:
        return json.loads(proc.stdout)
    except json.JSONDecodeError:
        print(proc.stdout, proc.stderr, file=sys.stderr)
        raise


# Things to delete, and things that must survive.
write(".npm/_cacache/c", 20)
write("work/proj/node_modules/x/m", 30)
write("work/proj/package.json", text="{}")
write("Documents/keep.txt", text="keep")
survivors = ["Documents/keep.txt", "work/proj/package.json"]
if MAC:
    write("Library/Caches/AppB/blob", 300)
    write("Library/Caches/com.apple.keep/k", 5)
    gone = ["Library/Caches/AppB", ".npm/_cacache", "work/proj/node_modules"]
    survivors += ["Library/Caches/com.apple.keep/k", "Library/Caches"]
    ids = ["caches/AppB", "npm", "dev:node_modules"]
else:
    write(".cache/AppB/blob", 300)
    write(".cache/huggingface/model", 30)
    write(".local/share/Trash/files/old.txt", text="old")
    write(".local/share/Trash/info/old.txt.trashinfo", text="[Trash Info]")
    write(".var/app/org.example.App/cache/c", 10)
    write(".var/app/org.example.App/data/save", 5)
    gone = [
        ".cache/AppB",
        ".npm/_cacache",
        "work/proj/node_modules",
        ".local/share/Trash/files/old.txt",
        ".local/share/Trash/info/old.txt.trashinfo",
        ".var/app/org.example.App/cache/c",
    ]
    survivors += [
        ".cache/huggingface/model",
        ".var/app/org.example.App/data/save",
        ".local/share/Trash/files",
        ".local/share/Trash/info",
        ".var/app/org.example.App/cache",
    ]
    ids = ["caches/AppB", "trash", "flatpak-caches", "npm", "dev:node_modules"]

try:
    scan = as_json(run("scan", "--json"))
    items = {i["id"]: i for i in scan["items"]}
    for want in ids:
        check(want in items and items[want]["cleanable"], f"scan offers {want}")
    check(items.get("caches/AppB", {}).get("safety") == "safe", "an app cache is safe")
    check(items.get("dev:node_modules", {}).get("safety") == "regenerable", "node_modules is regenerable")
    if not MAC:
        check(not any(i.startswith("caches/huggingface") for i in items), "model weights are not a cache")
        check(items.get("models/huggingface", {}).get("safety") == "review", "model weights are for review")
        check(not any(i.startswith("leftover/") for i in items), "no app containers outside macOS")
    check(isinstance(scan["system"], list), "system entries are listed")

    plan = as_json(run("plan", *ids, "--json"))
    check(plan["refused"] == [], "nothing refused while planning")
    planned = sorted(p["path"] for i in plan["items"] for p in i["paths"])

    applied = run("apply", plan["id"], "--json")
    result = as_json(applied)
    check(applied.returncode == 0, f"apply exits 0 (got {applied.returncode})")
    check(sorted(d["path"] for d in result["deleted"]) == planned, "apply deletes exactly the planned paths")
    check(result["deleted_bytes"] >= 300 << 20, "the measured size covers the 300 MiB cache")
    for rel in gone:
        check(not (HOME / rel).exists(), f"deleted {rel}")
    for rel in survivors:
        check((HOME / rel).exists(), f"kept {rel}")

    again = run("apply", plan["id"], "--json")
    check(again.returncode == 3 and as_json(again)["kind"] == "refused", "a plan runs only once")

    log = as_json(run("log", "--json"))
    check([e["plan"] for e in log["entries"]] == [plan["id"]], "the deletion is in the audit log")

    check(run("clean", "npm").returncode == 3, "clean refuses without a terminal")

    write(".npm/_cacache/c", 20)
    tampered = as_json(run("plan", "npm", "--json"))
    plan_file = HOME / ".local/state/disk-clean/plans" / f"{tampered['id']}.json"
    plan_file.write_text(plan_file.read_text().replace("_cacache", "_cacachf"))
    check(run("apply", tampered["id"], "--json").returncode == 3, "an edited plan is refused")
    check((HOME / ".npm/_cacache/c").exists(), "and nothing was deleted")

    # The whitelist: built-in protection, the user's own entries, and an entry added after planning.
    cache = "Library/Caches" if MAC else ".cache"
    builtin = "Library/Caches/CloudKit/db" if MAC else ".cache/pypoetry/virtualenvs/proj-py3.12/bin/python"
    write(builtin, 1)
    write(f"{cache}/Small/x", 1)
    write(f"{cache}/Keep/x", 1)
    added = run("whitelist", "add", f"~/{cache}/Keep", "--json")
    check(added.returncode == 0 and as_json(added)["added"] == [f"~/{cache}/Keep"], "whitelist add")
    caches = {i["id"]: i for i in as_json(run("scan", "--json"))["items"]}.get("caches", {})
    offered = " ".join(caches.get("paths", []))
    check("Small" in offered and "Keep" not in offered, "a whitelisted cache is not offered")
    check("CloudKit" not in offered and "pypoetry" not in offered, "built-in protection is not offered")
    check(caches.get("protected", 0) >= 2, f"the protected count is shown (got {caches.get('protected')})")

    write(f"{cache}/Late/x", 1)
    late = as_json(run("plan", "caches", "--json"))
    check(any(p["path"].endswith("/Late") for i in late["items"] for p in i["paths"]), "Late is in the plan")
    run("whitelist", "add", f"~/{cache}/Late")
    applied = run("apply", late["id"], "--json")
    result = as_json(applied)
    check(applied.returncode == 2, f"apply is partial when a planned path was whitelisted since (got {applied.returncode})")
    check(any(r["path"].endswith("/Late") and "whitelist" in r["reason"] for r in result["refused"]), "and says why")
    for rel in [builtin, f"{cache}/Keep/x", f"{cache}/Late/x"]:
        check((HOME / rel).exists(), f"kept {rel}")
    check(not (HOME / cache / "Small").exists(), "the rest of the plan was deleted")

    check(run("whitelist", "remove", "~/Library/Caches/CloudKit*" if MAC else "~/.cache/pypoetry/virtualenvs*").returncode == 3, "built-in protection cannot be removed")
    check(run("whitelist", "remove", f"~/{cache}/Keep").returncode == 0, "whitelist remove")
    wl_file = HOME / ".config/disk-clean/whitelist"
    wl_file.write_text(wl_file.read_text() + "relative/path\n")
    check(len(as_json(run("whitelist", "--json"))["warnings"]) == 1, "a bad line is reported, not ignored")
finally:
    shutil.rmtree(HOME, ignore_errors=True)

print(f"\n{len(failures)} failed" if failures else "\nall passed")
sys.exit(1 if failures else 0)
