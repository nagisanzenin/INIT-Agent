#!/usr/bin/env python3
"""evo: build and validate the evo-test-v1 task set (measurements/evo-test-protocol.md; lane EVO-TEST-PREP).

Python >= 3.9, stdlib only. Run it with /usr/bin/python3 -I from the repo root, validators niced in the light pool:

    INIT_SLOT_POOL=light bash ci/slot.sh nice -n 10 /usr/bin/python3 -I fixtures/evo-test-v1/tools/evo.py validate

    evo.py build    --out DIR [--only ID,...]                built task dirs (task.json v3, workspace.tar,
                                                             solution.tar, check/) -- DIR must be harness-only
    evo.py validate [--only ID,...] [--repeat 3] [--json]    pool-v1's validation plus the evo rules below
    evo.py manifest [--check]                                write (or verify) evo.json from the task sources
    evo.py list     [--group train|heldout|spare]            task ids, one per line (the runner's input)

This is a thin layer over fixtures/pool-v1/tools/pool.py: the same task source format (task.in.json, workspace/,
workspace.gen, solution/ or solve.sh, hidden/, partial/), the same deterministic tars and the same check runner
(pool-v1's poolcheck.py, copied byte-identical into each built check/). Only the module globals that name the task
directory, the pool id and the wall limit are rebound, and the source rule is replaced (every evo task is fresh).

Evo rules on top of pool-v1's (reference 1000 x3, no-op x3 equal and <= 500, empty < 1000, partial strictly
between, deterministic build, tamper of the first protected file detected):
  - every task protects at least one file, so tamper detection is always exercised;
  - the empty workspace scores < 300 (protocol: "empty < 300");
  - every check finishes in <= 30 s (pool-v1: 10 s);
  - task.in.json carries "evo": {"group": "train"|"heldout", "kind": <kind>, "spare": bool, "pair": <id>|null}
    and the prompt is owner-style: it names no file under hidden/ and no solution.

solution.tar is written only by `build` into the directory the caller names. The runner keeps that directory out
of every path a test home can read (protocol §4.3).
"""
from __future__ import annotations

import argparse
import json
import os
import re
import shutil
import sys
import tempfile

sys.dont_write_bytecode = True
EVO_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REPO = os.path.dirname(os.path.dirname(EVO_DIR))
sys.path.insert(0, os.path.join(REPO, "fixtures", "pool-v1", "tools"))
import pool as P  # noqa: E402

P.POOL_DIR = EVO_DIR
P.TASKS_DIR = os.path.join(EVO_DIR, "tasks")
P.POOL_ID = "evo-test-v1"
P.CHECK_WALL_MAX_S = 30.0
EMPTY_MAX = 300
GROUPS = ("train", "heldout")
KIND_RE = re.compile(r"^[a-z][a-z0-9-]{1,30}$")
HINT_RE = re.compile(r"hidden/|solution|check\.py|check\.sh|\bhint\b", re.I)


def evo_validate_source(t: dict) -> list:
    errs = []
    if (t.get("source") or {}).get("kind") != "fresh":
        errs.append("evo tasks are written fresh: source.kind must be fresh")
    if t.get("family") != "t":
        errs.append("evo tasks use family t (fresh terminal tasks)")
    e = t.get("evo")
    if not isinstance(e, dict) or set(e) != {"group", "kind", "spare", "pair"}:
        errs.append('task.in.json needs "evo": {"group", "kind", "spare", "pair"}')
        return errs
    if e["group"] not in GROUPS:
        errs.append("evo.group must be train or heldout")
    if not isinstance(e["kind"], str) or not KIND_RE.match(e["kind"]):
        errs.append("evo.kind must match %s" % KIND_RE.pattern)
    if not isinstance(e["spare"], bool):
        errs.append("evo.spare must be a boolean")
    if e["pair"] is not None and not os.path.isfile(os.path.join(P.TASKS_DIR, str(e["pair"]), "task.in.json")):
        errs.append("evo.pair %r is not a task" % e["pair"])
    if HINT_RE.search(t.get("prompt", "")):
        errs.append("the prompt names hidden checks or a solution (owner-style messages describe the task only)")
    if not t.get("protected"):
        errs.append("protect at least one workspace file (tamper detection must be exercised)")
    return errs


P.validate_source = evo_validate_source


def tasks(group: str | None = None) -> list:
    out = []
    for tid in P.task_ids():
        e = P.load_src(tid).get("evo") or {}
        if group is None or (group == "spare" and e.get("spare")) or (group in GROUPS and e.get("group") == group and not e.get("spare")):
            out.append(tid)
    return out


def validate(ids: list, repeat: int, toolchain: str | None, scratch: str) -> list:
    b1 = os.path.join(scratch, "b1")
    P.build_all(b1, ids)
    reports = P.validate(ids, repeat, toolchain, None, scratch, built=b1)
    for r in reports:
        td = os.path.join(b1, r["id"])
        sc, tamper, code, wall, err, _ = P.run_check(td, P.make_tar_empty(), toolchain, None, scratch)
        r["empty"] = sc
        if sc >= EMPTY_MAX:
            r["errors"].append("the empty workspace scores %d (must be < %d)" % (sc, EMPTY_MAX))
        if r["tamper_ok"] is None:
            r["errors"].append("no protected file: tamper is not exercised")
        e = P.load_src(r["id"]).get("evo") or {}
        r["group"], r["kind"], r["spare"] = e.get("group"), e.get("kind"), e.get("spare")
        r["ok"] = not r["errors"]
    return reports


def manifest() -> dict:
    rows = []
    for tid in P.task_ids():
        t = P.load_src(tid)
        e = t.get("evo") or {}
        rows.append({"id": tid, "title": t["title"], "group": e.get("group"), "kind": e.get("kind"),
                     "spare": e.get("spare"), "pair": e.get("pair"), "difficulty": t["difficulty"], "labels": t["labels"]})
    count = {}
    for x in rows:
        k = "spare" if x["spare"] else x["group"]
        count[k] = count.get(k, 0) + 1
    return {"v": 1, "set": "evo-test-v1", "doc": "measurements/evo-test-protocol.md", "runner": "pool-v1 poolcheck/1",
            "composition": count, "tasks": rows,
            "licence": "written fresh for INIT (INIT licence); no upstream source"}


def main(argv: list) -> int:
    ap = argparse.ArgumentParser(prog="evo.py", description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name in ("build", "validate", "manifest", "list"):
        p = sub.add_parser(name)
        p.add_argument("--only", default="")
        p.add_argument("--toolchain", default=None)
        p.add_argument("--repeat", type=int, default=3)
        p.add_argument("--json", action="store_true")
        if name == "build":
            p.add_argument("--out", required=True)
        if name == "manifest":
            p.add_argument("--check", action="store_true")
        if name == "list":
            p.add_argument("--group", default=None, choices=("train", "heldout", "spare"))
    a = ap.parse_args(argv[1:])
    only = [x for x in a.only.split(",") if x] or None
    try:
        if a.cmd == "list":
            print("\n".join(tasks(a.group)))
            return 0
        if a.cmd == "build":
            for p in P.build_all(a.out, only):
                print(p)
            return 0
        if a.cmd == "manifest":
            want = P.dumps(manifest())
            path = os.path.join(EVO_DIR, "evo.json")
            if a.check:
                have = open(path, "rb").read() if os.path.exists(path) else b""
                if have != want:
                    P.say("evo.json is stale: run evo.py manifest")
                    return 1
                return 0
            with open(path, "wb") as f:
                f.write(want)
            return 0
        if a.cmd == "validate":
            scratch = tempfile.mkdtemp(prefix="evo-validate-")
            try:
                reports = validate(only or P.task_ids(), a.repeat, a.toolchain, scratch)
            finally:
                P.make_writable(scratch)
                shutil.rmtree(scratch, ignore_errors=True)
            if a.json:
                sys.stdout.write(P.dumps(reports).decode("ascii"))
            else:
                for r in reports:
                    print("%-30s %s %-7s %-10s initial=%s solution=%s partial=%s empty=%d max_wall=%dms tamper=%s" % (
                        r["id"], "ok  " if r["ok"] else "FAIL", r["group"], r["kind"], r["initial"], r["solution"],
                        r["partial"], r["empty"], r["max_wall_ms"], r["tamper_ok"]))
                    for e in r["errors"]:
                        print("    - " + e)
                print("%d/%d tasks valid" % (sum(r["ok"] for r in reports), len(reports)))
            return 0 if all(r["ok"] for r in reports) else 1
    except P.PoolError as e:
        P.say("evo.py: %s" % e)
        return 1
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
