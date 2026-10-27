#!/usr/bin/python3 -I
"""EVO-TEST-PREP: the logic of tests/real/evo_test.sh, the quick evolution test of ADR-011 (reward-free self-extension).
Protocol and pre-registration: measurements/evo-test-protocol.md (PREREG below must equal its JSON block; selftest).
Tasks: fixtures/evo-test-v1 (tools/evo.py on pool-v1's format and runner).

  evo_checks.py selftest                         pure logic + a sandboxed-scorer test ($0; no Root, no network, no key)
  evo_checks.py ready [--bins DIR] [--json]      what exists vs what waits on the ADR-011 build (blocker list)
  evo_checks.py build-tasks --out DIR            built tasks WITHOUT solution.tar (reference scored 1000 first, sandboxed)
  evo_checks.py prep-ws --task-dir T --ws W      a fresh workspace from workspace.tar
  evo_checks.py next --ledger L --tasks DIR --phase N [--mode M]   the next run spec (JSON) or nothing
  evo_checks.py can-start --ledger L --spec F --mode M             rc 0 go · 5 budget/halt · 6 wait for off-peak · 7 disk
  evo_checks.py params|charter|kconfig ...       per-run test-home config (money ceilings, read_deny, [extend])
  evo_checks.py run-one ...                      with the Root running: workspace.set, the owner line, the guards
  evo_checks.py score --task-dir T --ws W --out F        the check, sandboxed (no network, writes only to its scratch)
  evo_checks.py attrib --rh RH --ih IH --since T [--until T]   capabilities created and used (ADR-011 sources, else zone)
  evo_checks.py record|classify|report|freeze|redact|mock-script ...

Money: every run has a REAL cap (PREREG run_cap_musd); a guard prices the Root's egress usage at DeepSeek's list price
every 5 s and reads the balance every 5 min (real); at the cap it freezes spend (charter ceilings := spent), interrupts
and /stops. The program cap ($2.50 by balance) is a balance floor checked before and during every run, with phase 2's
envelope reserved before phase 1 may spend. This script never reads the key and never prints the owner's e-mail or
git identity. It never reads a task's solution: built task dirs hold no solution.tar."""
import argparse
import calendar
import glob
import hashlib
import json
import os
import random
import re
import shutil
import signal
import socket
import subprocess
import sys
import tempfile
import time

sys.dont_write_bytecode = True
HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, HERE)
import checks as S  # noqa: E402  S-REAL plumbing: Owner (attach driver), http over unix sockets, lineage
import checks_m7 as M  # noqa: E402  REAL-M7: priced usage, Guard, balance, charter/kconfig text, redaction
sys.path.insert(0, os.path.join(REPO, "fixtures", "evo-test-v1", "tools"))
import evo as E  # noqa: E402  the task set (pool-v1 format and runner)
P = E.P

MUSD = 1_000_000
PROTOCOL = os.path.join(REPO, "measurements", "evo-test-protocol.md")

# v1 pre-registration, kept for the record (protocol §7); superseded by v2 before any real run.
PREREG_V1 = {
    "version": "evo-test-v1/1",
    "model": "deepseek-flash",
    "program_cap_real_musd": 2_500_000,
    "reserve_musd": 100_000,
    "p0_envelope_musd": 900_000,
    "p2_envelope_musd": 800_000,
    "run_cap_musd": {"p0": 50_000, "p1": 100_000, "p2": 50_000},
    "run_wall_s": {"p0": 1200, "p1": 2400, "p2": 1200},
    "p1_idle_window_s": 600,
    "p1_idle_quiet_s": 120,
    "p0_runs": 2,
    "p1_attempts": 3,
    "p2_runs": 2,
    "solved_permille": 1000,
    "min_set": 2,
    "max_set": 4,
    "p0_early_stop": True,
    "infra_reruns": 1,
    "p2_seed": 20261022,
    "kind_order": ["spec", "nav", "debug", "bigfile"],
    "retry_preface": ("The previous attempt at this task did not meet the requirements. "
                      "The workspace has been reset to its original state."),
    "success": {"min_new_caps": 1, "min_extra_solved_runs": 2, "min_attributed_tasks": 1},
    "kernel_task_usd": "0.20",
    "start_min_free_mb": {"install": 7000, "p0": 6500, "p1": 8000, "p2": 6500},
    "watch_min_free_mb": 5500,
    "mem_cap_mb": 4096,
}
# v2 pre-registration (owner: minimal), the record of the stopped bigfile run (protocol §10).
# One kind, 1 training + 2 held-out tasks named here, one flexible spare; $1.00 real by balance; strict verdict only.
PREREG_V2 = {
    "version": "evo-test-v1/2 (owner: minimal)",
    "registered": "2026-10-22",
    "model": "deepseek-flash",
    "kind": "bigfile",
    "tasks": {"train": ["evo-big-client"], "heldout": ["evo-big-rules", "evo-big-schema"], "spares": ["evo-big-forms"]},
    "program_cap_real_musd": 1_000_000,
    "reserve_musd": 50_000,
    "p0_envelope_musd": 340_000,
    "p2_envelope_musd": 320_000,
    "run_cap_musd": {"p0": 40_000, "p1": 80_000, "p2": 40_000},
    "run_wall_s": {"p0": 1200, "p1": 2400, "p2": 1200},
    "p1_idle_window_s": 600,
    "p1_idle_quiet_s": 120,
    "p0_runs": 2,
    "p1_attempts": 3,
    "p2_runs": 2,
    "solved_permille": 1000,
    "p0_early_stop": True,
    "infra_reruns": 1,
    "p2_seed": 20261022,
    "retry_preface": ("The previous attempt at this task did not meet the requirements. "
                      "The workspace has been reset to its original state."),
    "success": {"verdict": "evolved_and_solved", "min_new_caps": 1, "min_extra_solved_runs": 2,
                "min_attributed_tasks": 1, "evidence": "root"},
    "kernel_task_usd": "0.20",
    "start_min_free_mb": {"install": 7000, "p0": 6500, "p1": 8000, "p2": 6500},
    "watch_min_free_mb": 5500,
    "mem_cap_mb": 4096,
}
# v3 pre-registration (the tickets candidates), the record: genesis solved all five (protocol §11).
# Calibration keeps a candidate only if genesis fails it 2/2 AND the control (a harness with the mechanism) solves it.
PREREG_V3 = {
    "version": "evo-test-v1/3 (owner: missing mechanism)",
    "registered": "2026-10-22",
    "model": "deepseek-flash",
    "kind": "tickets",
    "mechanism": "task-tracking-and-completion-check",
    "tasks": {"train": ["evo-tix-library", "evo-tix-garage"], "heldout": ["evo-tix-pantry", "evo-tix-clinic", "evo-tix-studio"],
              "spares": []},
    "need": {"train": 1, "heldout": 2},
    "control": {"harness": "claude-code", "version": "2.1.261", "endpoint": "https://api.deepseek.com/anthropic",
                "todo_tools": True, "runs": 2, "stop_at_first_solve": True},
    "program_cap_real_musd": 2_500_000,
    "reserve_musd": 200_000,
    "p0_envelope_musd": 1_400_000,
    "p1_envelope_musd": 400_000,
    "p2_envelope_musd": 500_000,
    "run_cap_musd": {"p0": 60_000, "p1": 120_000, "p2": 60_000},
    "run_wall_s": {"p0": 1200, "p1": 2400, "p2": 1200},
    "p1_idle_window_s": 600,
    "p1_idle_quiet_s": 120,
    "p0_runs": 2,
    "p1_attempts": 3,
    "p2_runs": 2,
    "solved_permille": 1000,
    "p0_early_stop": False,
    "infra_reruns": 1,
    "p2_seed": 20261022,
    "retry_preface": ("The previous attempt at this task did not meet the requirements. "
                      "The workspace has been reset to its original state."),
    "success": {"verdict": "evolved_and_solved", "min_new_caps": 1, "min_extra_solved_runs": 2,
                "min_attributed_tasks": 1, "evidence": "root", "mechanism_review": "blind subagent, yes required"},
    "kernel_task_usd": "0.20",
    "start_min_free_mb": {"install": 7000, "p0": 6500, "p1": 8000, "p2": 6500},
    "watch_min_free_mb": 5500,
    "mem_cap_mb": 4096,
}
# v3.1 pre-registration (candidates A), the record: no 1 + 2 set, the control capped out (protocol §11.6-§11.7).
# Calibration keeps a candidate only if genesis fails it 2/2 AND the control (a harness with the mechanism) solves it.
PREREG_V31 = {
    "version": "evo-test-v1/3.1 (owner: missing mechanism, candidates A)",
    "registered": "2026-10-22",
    "model": "deepseek-flash",
    "kind": "long-work",
    "mechanism": "task-tracking-and-completion-check",
    "tasks": {"train": ["evo-big-client", "evo-nav-units"],
              "heldout": ["evo-big-forms", "evo-nav-tz", "evo-spec-sched", "evo-dbg-sheet", "evo-spec-tmpl"], "spares": []},
    "need": {"train": 1, "heldout": 2},
    "control": {"harness": "claude-code", "version": "2.1.261", "endpoint": "https://api.deepseek.com/anthropic",
                "todo_tools": True, "runs": 2, "stop_at_first_solve": True},
    "program_cap_real_musd": 2_500_000,
    "reserve_musd": 200_000,
    "p0_envelope_musd": 1_400_000,
    "p1_envelope_musd": 400_000,
    "p2_envelope_musd": 500_000,
    "run_cap_musd": {"p0": 60_000, "p1": 120_000, "p2": 60_000},
    "run_wall_s": {"p0": 1200, "p1": 2400, "p2": 1200},
    "p1_idle_window_s": 600,
    "p1_idle_quiet_s": 120,
    "p0_runs": 2,
    "p1_attempts": 3,
    "p2_runs": 2,
    "solved_permille": 1000,
    "p0_early_stop": False,
    "infra_reruns": 1,
    "p2_seed": 20261022,
    "retry_preface": ("The previous attempt at this task did not meet the requirements. "
                      "The workspace has been reset to its original state."),
    "success": {"verdict": "evolved_and_solved", "min_new_caps": 1, "min_extra_solved_runs": 2,
                "min_attributed_tasks": 1, "evidence": "root", "mechanism_review": "blind subagent, yes required"},
    "kernel_task_usd": "0.20",
    "start_min_free_mb": {"install": 7000, "p0": 6500, "p1": 8000, "p2": 6500},
    "watch_min_free_mb": 5500,
    "mem_cap_mb": 4096,
}
# v3.2 pre-registration (family B, implicit-prose changes; protocol §11.7 carries the same JSON; selftest).
# Calibration keeps a candidate only if genesis fails it 2/2 AND the control (a harness with the mechanism) solves it.
PREREG = {
    "version": "evo-test-v1/3.2 (owner: missing mechanism, family B)",
    "registered": "2026-10-22",
    "model": "deepseek-flash",
    "kind": "prose",
    "mechanism": "task-tracking-and-completion-check",
    "tasks": {"train": ["evo-prose-library", "evo-prose-garage"],
              "heldout": ["evo-prose-pantry", "evo-prose-clinic", "evo-prose-studio"], "spares": []},
    "need": {"train": 1, "heldout": 2},
    "control": {"harness": "claude-code", "version": "2.1.261", "endpoint": "https://api.deepseek.com/anthropic",
                "todo_tools": True, "runs": 2, "stop_at_first_solve": True},
    "program_cap_real_musd": 2_500_000,
    "reserve_musd": 110_000,
    "p0_envelope_musd": 1_280_000,
    "p1_envelope_musd": 300_000,
    "p2_envelope_musd": 800_000,
    "run_cap_musd": {"p0": 100_000, "p1": 150_000, "p2": 100_000},
    "run_wall_s": {"p0": 1200, "p1": 2400, "p2": 1200},
    "p1_idle_window_s": 600,
    "p1_idle_quiet_s": 120,
    "p0_runs": 2,
    "p1_attempts": 2,
    "p2_runs": 2,
    "solved_permille": 1000,
    "p0_early_stop": False,
    "infra_reruns": 1,
    "p2_seed": 20261022,
    "retry_preface": ("The previous attempt at this task did not meet the requirements. "
                      "The workspace has been reset to its original state."),
    "success": {"verdict": "evolved_and_solved", "min_new_caps": 1, "min_extra_solved_runs": 2,
                "min_attributed_tasks": 1, "evidence": "root", "mechanism_review": "blind subagent, yes required"},
    "kernel_task_usd": "0.20",
    "start_min_free_mb": {"install": 7000, "p0": 6500, "p1": 8000, "p2": 6500},
    "watch_min_free_mb": 5500,
    "mem_cap_mb": 4096,
}
ARMS = ("genesis", "evolved", "twin")
ADR011_MARKERS = {
    "root.change_open": ("init-root", b"change_open"),
    "root.cap_use": ("init-root", b"cap_use"),
    "root.extend_mode": ("init-root", b"extend.mode"),
    "root.egress_caps": ("init-root", b"x-egress-caps"),
    "kernel.change_open": ("init-kernel", b"change.open"),
    "kernel.cap_trailer": ("init-kernel", b"Measure"),   # the trailer table holds "Cap" and "Measure" together
}


# ============================================================================================ small helpers


def iso(t):
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(t))


def parse_iso(s):
    try:
        return calendar.timegm(time.strptime(s[:19], "%Y-%m-%dT%H:%M:%S"))
    except (TypeError, ValueError):
        return None


def wj(p, v):
    tmp = p + ".tmp"
    with open(tmp, "w") as f:
        json.dump(v, f, sort_keys=True, indent=1)
        f.write("\n")
    os.replace(tmp, p)


def jload(p, default=None):
    try:
        with open(p) as f:
            return json.load(f)
    except (OSError, ValueError):
        return default


def free_mb(path="/"):
    s = os.statvfs(path)
    return s.f_bavail * s.f_frsize // (1 << 20)


def rm_tree(p):
    """Removes a tree this script created, by exact path (read-only dirs made writable first)."""
    if os.path.lexists(p):
        P.make_writable(p)
        if os.path.isdir(p) and not os.path.islink(p):
            shutil.rmtree(p)
        else:
            os.unlink(p)


# ============================================================================================ tasks and scoring


def sandbox_profile(out_dir):
    """Agent-written code runs inside the check: no network, writes only under the scoring scratch dir."""
    o = os.path.realpath(out_dir).replace("\\", "\\\\").replace('"', '\\"')
    return ("(version 1)\n(allow default)\n(deny network*)\n(deny file-write*)\n"
            f'(allow file-write* (subpath "{o}") (literal "/dev/null") (literal "/dev/tty") (regex #"^/dev/fd/"))\n')


def run_check_sandboxed(task_dir, ws_tar, scratch, timeout_s=None):
    """pool.run_check's protocol (fresh extraction, check/ copied, clean env), under sandbox-exec.
    Returns {score, tamper, exit, wall_ms, last, stderr_tail}."""
    out = tempfile.mkdtemp(prefix="evo-score-", dir=scratch)
    try:
        ws = os.path.join(out, "ws")
        P.extract_tar(ws_tar, ws)
        shutil.copytree(os.path.join(task_dir, "check"), os.path.join(out, "check"))
        for d in ("tmp", "home"):
            os.makedirs(os.path.join(out, d))
        task = jload(os.path.join(task_dir, "task.json"))
        env = {"PATH": "/usr/bin:/bin", "HOME": os.path.join(out, "home"), "TMPDIR": os.path.join(out, "tmp"),
               "LANG": "C", "INIT_TASK_ID": task["id"], "INIT_CHECK_DIR": os.path.join(out, "check")}
        argv = ["/usr/bin/sandbox-exec", "-p", sandbox_profile(out), os.path.join(out, "check", "run")]
        t0 = time.monotonic()
        try:
            p = subprocess.run(argv, cwd=ws, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, timeout=timeout_s or task["check"]["timeout_s"],
                               start_new_session=True)
            code, so, se = p.returncode, p.stdout, p.stderr
        except subprocess.TimeoutExpired as e:
            code, so, se = None, e.stdout or b"", e.stderr or b""
        wall = time.monotonic() - t0
        lines = so.decode("utf-8", "replace").strip().splitlines()
        last = lines[-1] if lines else ""
        score, tamper = 0, False
        try:
            j = json.loads(last)
            tamper = j.get("tamper") is True
            if code == 0 and isinstance(j.get("score_permille"), int):
                score = j["score_permille"]
        except ValueError:
            pass
        return {"score": score, "tamper": tamper, "exit": code, "wall_ms": int(wall * 1000), "last": last[:200],
                "stderr_tail": se.decode("utf-8", "replace")[-1500:]}
    finally:
        rm_tree(out)


WS_MAX_BYTES = 512 << 20


def score_ws(task_dir, ws, scratch):
    size = 0
    for dp, dn, fn in os.walk(ws):
        dn[:] = [d for d in dn if not os.path.islink(os.path.join(dp, d))]
        size += sum(os.lstat(os.path.join(dp, n)).st_size for n in fn)
    if size > WS_MAX_BYTES:
        return {"score": 0, "tamper": False, "exit": None, "wall_ms": 0, "last": "", "stderr_tail": f"{size} bytes",
                "refused": "workspace_too_large"}
    try:
        tar = P.make_tar(ws)
    except P.PoolError as e:  # a link or special file in the workspace: the check would not run it either
        return {"score": 0, "tamper": False, "exit": None, "wall_ms": 0, "last": "", "stderr_tail": str(e)[-300:],
                "refused": "workspace_shape"}
    return run_check_sandboxed(task_dir, tar, scratch)


def build_tasks(out, ids=None):
    """Builds the set into a scratch dir, scores each reference solution sandboxed (must be 1000) and the initial
    workspace, then copies task.json, workspace.tar and check/ into out/<id>/ (no solution.tar anywhere in out)."""
    os.makedirs(out, exist_ok=True)
    scratch = tempfile.mkdtemp(prefix="evo-build-")
    index = {"v": 1, "built": iso(time.time()), "tasks": {}}
    try:
        b = os.path.join(scratch, "b")
        ids = ids or P.task_ids()
        P.build_all(b, ids)
        for tid in ids:
            td = os.path.join(b, tid)
            with open(os.path.join(td, "solution.tar"), "rb") as f:
                ref = run_check_sandboxed(td, f.read(), scratch)
            with open(os.path.join(td, "workspace.tar"), "rb") as f:
                ini = run_check_sandboxed(td, f.read(), scratch)
            if ref["score"] != 1000:
                raise SystemExit(f"{tid}: the reference scores {ref['score']} sandboxed: {ref['stderr_tail'][-400:]}")
            dst = os.path.join(out, tid)
            rm_tree(dst)
            os.makedirs(dst)
            for name in ("task.json", "workspace.tar"):
                shutil.copyfile(os.path.join(td, name), os.path.join(dst, name))
            shutil.copytree(os.path.join(td, "check"), os.path.join(dst, "check"))
            src = P.load_src(tid)
            e = src.get("evo") or {}
            with open(os.path.join(dst, "task.json"), "rb") as f:
                tj = f.read()
            index["tasks"][tid] = {"group": e.get("group"), "kind": e.get("kind"), "spare": e.get("spare"),
                                   "pair": e.get("pair"), "task_sha256": hashlib.sha256(tj).hexdigest(),
                                   "initial": ini["score"], "reference": ref["score"]}
        wj(os.path.join(out, "index.json"), index)
    finally:
        rm_tree(scratch)
    for dp, dn, fn in os.walk(out):
        if "solution.tar" in fn:
            raise SystemExit(f"solution.tar found under {dp}")
    return index


# ============================================================================================ schedule and classification


def ledger_new(mode):
    return {"v": 1, "mode": mode, "prereg_sha256": prereg_sha(), "program": {}, "runs": {}, "sets": None,
            "p0": None, "p1": {"started": False, "done": False, "e1": False}, "halt": None}


def prereg_sha():
    return hashlib.sha256(json.dumps(PREREG, sort_keys=True).encode()).hexdigest()


def primaries(index, group):
    """The pre-registered tasks of a group (train, heldout), in their registered order."""
    return [t for t in PREREG["tasks"][group] if t in index["tasks"]]


def spare_list(index):
    return [t for t in PREREG["tasks"]["spares"] if t in index["tasks"]]


def spare_assign(index, led):
    """{spare: (group, the dropped primary it replaces)}: dropped primaries in registered order (training first),
    spares in registered order."""
    dropped = [(g, t) for g in ("train", "heldout") for t in primaries(index, g) if p0_status(led, t) == "dropped"]
    return {sp: gt for gt, sp in zip(dropped, spare_list(index))}


def runs_of(led, phase, task, arm=None):
    """Valid (non-infra) runs of a task in a phase, in order."""
    out = [r for r in led["runs"].values() if r.get("phase") == phase and r.get("task") == task
           and (arm is None or r.get("arm") == arm) and not r.get("infra")]
    return sorted(out, key=lambda r: r.get("seq", 0))


def infra_count(led, tag):
    return sum(1 for r in led["runs"].values() if r.get("base_tag") == tag and r.get("infra"))


def spec_for(phase, task, arm, k, restore, led, kind="run"):
    base = f"p{phase}-{task}-{arm}-{'a' if phase == 1 else 'r'}{k}"
    n = infra_count(led, base)
    ph = f"p{phase}"
    return {"tag": base + ("" if n == 0 else f"-x{n}"), "base_tag": base, "phase": phase, "task": task, "arm": arm,
            "k": k, "restore": restore, "cap_musd": PREREG["run_cap_musd"][ph], "wall_s": PREREG["run_wall_s"][ph],
            "idle_s": PREREG["p1_idle_window_s"] if phase == 1 else 0, "kind": kind}


def pending_base(led, base):
    """True when this base tag still needs a run: no valid run, and infra reruns not exhausted."""
    done = any(r.get("base_tag") == base and not r.get("infra") for r in led["runs"].values())
    return not done and infra_count(led, base) <= PREREG["infra_reruns"]


def p0_plan(index, led):
    """Primaries x runs (run 1 of every task, then run 2), then the spares that replace dropped primaries."""
    out = []
    prim = primaries(index, "train") + primaries(index, "heldout")
    for k in range(1, PREREG["p0_runs"] + 1):
        out += [(t, k) for t in prim]
    for sp in spare_assign(index, led):
        out += [(sp, k) for k in range(1, PREREG["p0_runs"] + 1)]
    return out


def p0_status(led, task):
    """dropped as soon as genesis scores 1000 once; beyond after p0_runs runs all below 1000; else pending."""
    rs = runs_of(led, 0, task, "genesis")
    if any(r.get("score", 0) >= PREREG["solved_permille"] for r in rs):
        return "dropped"
    return "beyond" if len(rs) >= PREREG["p0_runs"] else "pending"


def p0_early_stop(index, led):
    """After run 1 of every primary: more primaries already dropped than spares exist -> stop before run 2
    (protocol §10.2). Returns the reason, or None."""
    if not PREREG["p0_early_stop"]:
        return None
    prim = primaries(index, "train") + primaries(index, "heldout")
    if any(not runs_of(led, 0, t) for t in prim):
        return None
    dropped = [t for t in prim if p0_status(led, t) == "dropped"]
    if len(dropped) > len(spare_list(index)):
        return f"{len(dropped)} of {len(prim)} tasks solved by genesis on run 1, {len(spare_list(index))} spare(s)"
    return None


def classify(index, led):
    """The kept sets after phase 0: primaries genesis failed on both runs, plus the spares that replace dropped ones
    (if they too are beyond capability). Proceed only with the registered sizes. None while a needed run is pending."""
    prim = primaries(index, "train") + primaries(index, "heldout")
    if any(p0_status(led, t) == "pending" for t in prim):
        return None
    assign = spare_assign(index, led)
    if any(p0_status(led, sp) == "pending" for sp in assign):
        return None
    if PREREG.get("control"):
        if any(control_status(led, t) == "pending" for t in prim if p0_status(led, t) == "beyond"):
            return None
    sets, notes = {}, []
    for g in ("train", "heldout"):
        kept = [t for t in primaries(index, g) if p0_status(led, t) == "beyond"
                and (not PREREG.get("control") or control_status(led, t) == "solved")]
        if PREREG.get("control"):
            notes += [f"{t}: genesis failed 2/2 but the control never solved it" for t in primaries(index, g)
                      if p0_status(led, t) == "beyond" and control_status(led, t) == "unsolved"]
            kept = kept[:PREREG.get("need", {}).get(g, len(kept))]
        for sp, (gg, t) in assign.items():
            if gg == g and p0_status(led, sp) == "beyond":
                kept.append(sp)
                notes.append(f"{sp} replaces {t} ({g})")
        sets[g] = kept
        notes += [f"{t} dropped: genesis scored 1000 in phase 0" for t in primaries(index, g) if p0_status(led, t) == "dropped"]
    need = PREREG.get("need") or {g: len(PREREG["tasks"][g]) for g in ("train", "heldout")}
    ok = all(len(sets[g]) >= need[g] for g in need)
    return {"sets": sets, "proceed": ok, "notes": notes,
            "why": None if ok else f"kept {({g: len(sets[g]) for g in sets})}, need {need}: genesis solves the set"}


def control_status(led, task):
    """solved (a control run scored 1000) · unsolved (all registered control runs below 1000) · pending."""
    rs = runs_of(led, 0, task, "control")
    if any(r.get("score", 0) >= PREREG["solved_permille"] for r in rs):
        return "solved"
    return "unsolved" if len(rs) >= PREREG["control"]["runs"] else "pending"


def next_control(index, led):
    """Control runs only on tasks genesis failed 2/2, stop at a task's first solve (protocol §11.3). Order (orchestrator,
    2026-10-22 21:20): the training group first, then the held-out group; within a group the clearest genesis failure
    first (lowest best genesis score, ties in registered order). A group whose selection is decided gets no more runs,
    and nothing runs once a 1 + 2 set can no longer exist."""
    if led.get("halt"):
        return None
    need = PREREG.get("need") or {}
    for g in ("train", "heldout"):
        cands = [t for t in primaries(index, g) if p0_status(led, t) == "beyond"]
        solved = [t for t in cands if control_status(led, t) == "solved"]
        open_ = [t for t in cands if control_status(led, t) == "pending"]
        if g in need and len(solved) + len(open_) < need[g]:
            return None   # this group can no longer reach its size: no further control spend
    for g in ("train", "heldout"):
        cands = [t for t in primaries(index, g) if p0_status(led, t) == "beyond"]
        if g in need and sum(1 for t in cands if control_status(led, t) == "solved") >= need[g]:
            continue
        order = sorted(cands, key=lambda t: (max(r.get("score", 0) for r in runs_of(led, 0, t, "genesis")),
                                             primaries(index, g).index(t)))
        for t in order:
            if control_status(led, t) != "pending":
                continue
            k = len(runs_of(led, 0, t, "control")) + 1
            sp = spec_for(0, t, "control", k, "none", led)
            if pending_base(led, sp["base_tag"]):
                return sp
    return None


def p2_order(heldout):
    """Held-out runs, interleaved: run 1 of every task, then run 2; per task the arm order alternates (ABBA), from a
    fixed seeded shuffle of the task order (protocol §3.3)."""
    tasks = list(heldout)
    random.Random(PREREG["p2_seed"]).shuffle(tasks)
    out = []
    for k in range(1, PREREG["p2_runs"] + 1):
        for i, t in enumerate(tasks):
            arms = ("evolved", "twin") if (i + k) % 2 == 0 else ("twin", "evolved")
            out += [(t, a, k) for a in arms]
    return out


def next_spec(index, led, phase):
    if led.get("halt"):
        return None
    if phase == 0:
        for t, k in p0_plan(index, led):
            base = f"p0-{t}-genesis-r{k}"
            if p0_status(led, t) == "dropped":   # solved once: dropped, no further run
                continue
            if pending_base(led, base):
                return spec_for(0, t, "genesis", k, "g0", led)
        return None
    if not led.get("sets"):
        return None
    if phase == 1:
        if led["p1"].get("done"):
            return None
        for t in led["sets"]["train"]:
            rs = runs_of(led, 1, t, "evolved")
            if any(r.get("score", 0) >= PREREG["solved_permille"] for r in rs) or len(rs) >= PREREG["p1_attempts"]:
                continue
            k = len(rs) + 1
            restore = "keep" if led["p1"].get("started") else "g0"
            return spec_for(1, t, "evolved", k, restore, led)
        return None
    if phase == 2:
        if not led["p1"].get("e1"):
            return None
        for t, arm, k in p2_order(led["sets"]["heldout"]):
            base = f"p2-{t}-{arm}-r{k}"
            if pending_base(led, base):
                return spec_for(2, t, arm, k, "e1" if arm == "evolved" else "g0", led)
        return None
    raise SystemExit(f"unknown phase {phase}")


def attempt_line(prompt, phase, k):
    if phase == 1 and k > 1:
        return PREREG["retry_preface"] + " " + prompt
    return prompt


# ============================================================================================ money gates


def spent_program(led):
    """Program spend so far: the larger of the priced estimate (sum over runs) and the balance drop."""
    priced = sum(int(r.get("real_est_musd") or 0) for r in led["runs"].values())
    pr = led.get("program") or {}
    drop = (pr.get("bal0_musd") - pr.get("bal_last_musd")) if pr.get("bal0_musd") and pr.get("bal_last_musd") else 0
    return max(priced, drop)


def spent_phase(led, phase):
    return sum(int(r.get("real_est_musd") or 0) for r in led["runs"].values() if r.get("phase") == phase)


def budget_gate(led, spec):
    """(ok, why): may this run start without breaking the program cap, phase 2's reservation or phase 0's envelope."""
    cap = PREREG["program_cap_real_musd"]
    spent = spent_program(led)
    future = PREREG["reserve_musd"] + (PREREG["p2_envelope_musd"] - spent_phase(led, 2) if spec["phase"] < 2 else 0)
    if spec["phase"] == 2:
        future = PREREG["reserve_musd"]
    if spent + spec["cap_musd"] + max(0, future) > cap:
        return False, f"program cap: spent {spent} + run cap {spec['cap_musd']} + reserved {future} > {cap} µUSD"
    if spec["phase"] == 1 and PREREG.get("p1_envelope_musd") and spent_phase(led, 1) + spec["cap_musd"] > PREREG["p1_envelope_musd"]:
        return False, f"phase 1 envelope: {spent_phase(led, 1)} + {spec['cap_musd']} > {PREREG['p1_envelope_musd']} µUSD"
    if spec["phase"] == 0 and spent_phase(led, 0) + spec["cap_musd"] > PREREG["p0_envelope_musd"]:
        return False, f"phase 0 envelope: {spent_phase(led, 0)} + {spec['cap_musd']} > {PREREG['p0_envelope_musd']} µUSD"
    return True, "ok"


def offpeak_ok(now, wall_s):
    return all(M.window_at(now + 60 * k) == "offpeak" for k in range(0, int(wall_s // 60) + 1))


# ============================================================================================ test-home config


def params(mode, rh, repo, x, cap_musd, mock_port, toolchain, devdir, extend):
    spent_sealed, spent_zone = M.pools_spent(rh) if rh and os.path.isdir(os.path.join(rh, "egress")) else (0, 0)
    head = M.cent_up(cap_musd * 2) + M.PARALLEL * M.RESERVATION_MUSD   # Root bills at peak: 2x the off-peak real price
    zone = M.cent_up(spent_zone) + head
    reserve = M.cent_up(spent_sealed) + 10_000
    base = os.path.join(repo, "target", "evo")
    deny = [os.path.join(repo, "root"), os.path.join(repo, ".git"), os.path.join(repo, "fixtures"),
            os.path.join(repo, "tests"), os.path.join(repo, "measurements"), os.path.join(base, f"{x}.tasks"),
            os.path.join(base, f"{x}.snap")]
    return {"mode": mode, "line": "evo", "cap_real_musd": cap_musd, "reserve_musd": reserve, "zone_cap_musd": zone,
            "ceiling_musd": reserve + zone, "read_deny": deny, "mock_port": mock_port, "toolchain": toolchain,
            "devdir": devdir, "task_musd": int(float(PREREG["kernel_task_usd"]) * MUSD), "gate_musd": 100_000,
            "extend": extend}


def charter_text(p):
    t = M.charter_text(p)
    t = re.sub(r'(?m)^text = ".*"$', f'text = "INIT test home ({p["mode"]})."', t, count=1)
    if p.get("extend") == "free":
        t += '\n[extend]\nmode = "free"\n'
    return t


def kconfig_text(p):
    return M.kconfig_text(p)


def freeze_params(p, rh):
    return M.frozen(p, rh)


def apply_charter(rh, home, p):
    with open(os.path.join(rh, "charter.toml"), "w") as f:
        f.write(charter_text(p))
    os.chmod(os.path.join(rh, "charter.toml"), 0o600)
    env = {"HOME": home, "INIT_ROOT_HOME": rh, "PATH": "/usr/bin:/bin"}
    r = subprocess.run([os.path.join(rh, "bin/init-root"), "charter", "check", "--apply", "--yes"], capture_output=True,
                       text=True, env=env, timeout=60)
    return r.returncode, r.stderr[-500:]


# ============================================================================================ Root plumbing


def owner_request(rh, cmd, args, timeout=10.0):
    """One owner-socket request (root/link.rs `request`): ICJ line in, one response line out."""
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(timeout)
    try:
        s.connect(os.path.join(rh, "run/owner.sock"))
        s.sendall((json.dumps({"args": args, "cmd": cmd, "id": "evo-1", "v": 1}, sort_keys=True, separators=(",", ":"))
                   + "\n").encode())
        buf = b""
        while not buf.endswith(b"\n") and len(buf) < (1 << 20):
            c = s.recv(65536)
            if not c:
                break
            buf += c
        return json.loads(buf.decode("utf-8", "replace") or "{}")
    finally:
        s.close()


def root_status(rh, home):
    env = {"HOME": home, "INIT_ROOT_HOME": rh, "PATH": "/usr/bin:/bin"}
    try:
        r = subprocess.run([os.path.join(rh, "bin/init-root"), "status", "--json"], capture_output=True, text=True,
                           env=env, timeout=20)
        return json.loads(r.stdout or "{}")
    except (OSError, ValueError, subprocess.TimeoutExpired):
        return {}


def set_workspace(a, ws, timeout=180):
    """workspace.set (as the TUI does at attach), then wait until the live body runs with that workspace."""
    real = os.path.realpath(ws)
    t_set = time.time()
    r = owner_request(a.rh, "workspace.set", {"cwd": real})
    res = {"ok": bool(r.get("ok")), "refused": ((r.get("result") or {}).get("refused")), "error": (r.get("error") or {}).get("code")}
    if not r.get("ok") or res["refused"]:
        return False, res
    ready = os.path.join(a.ih, "state/run/ready")
    end = time.time() + timeout
    while time.time() < end:
        st = root_status(a.rh, a.home)
        acc = st.get("access") or {}
        if (acc.get("workspace") == real and acc.get("restart_pending") is False and st.get("slot") == "running"
                and os.path.exists(ready) and os.path.getmtime(ready) >= t_set - 1):
            res["applied_s"] = int(time.time() - t_set)
            return True, res
        time.sleep(1)
    res["timeout"] = True
    return False, res


class Ns:
    def __init__(self, **kw):
        self.__dict__.update(kw)


class RawOwner:
    """An owner-channel client on ROOT_HOME/run/owner.sock, as the `init-agent` TUI is: one `line` frame per owner message,
    newlines included. (`init-root attach` turns every stdin line into its own frame, so a multi-line task would reach
    the body as a first line plus steers.) Frames go to <run>/attach.jsonl; same reading API as checks.Owner."""

    def __init__(self, a):
        import queue
        import threading
        self.s = socket.socket(socket.AF_UNIX)
        self.s.connect(os.path.join(a.rh, "run/owner.sock"))
        self.rf = self.s.makefile("r", encoding="utf-8", newline="\n")
        self.q = queue.Queue()
        self.busy = False
        self.out = open(os.path.join(a.run, "attach.jsonl"), "a")
        self.t0 = time.monotonic()
        self._write({"client": "evo_checks", "replay": False, "t": "attach"})
        threading.Thread(target=self._reader, daemon=True).start()
        time.sleep(0.3)

    def _write(self, obj):
        self.s.sendall((json.dumps(obj, sort_keys=True, separators=(",", ":"), ensure_ascii=False) + "\n").encode("utf-8"))

    def _reader(self):
        try:
            for line in self.rf:
                self.q.put(line.rstrip("\n"))
        except (OSError, ValueError):
            pass
        self.q.put(None)

    def send(self, line):
        self.out.write(json.dumps({"ms": int((time.monotonic() - self.t0) * 1000), "sent": line[:300],
                                   "sent_sha256": hashlib.sha256(line.encode()).hexdigest()}) + "\n")
        self._write({"t": "line", "text": line})

    def interrupt(self):
        self.out.write(json.dumps({"ms": int((time.monotonic() - self.t0) * 1000), "sent": "<interrupt>"}) + "\n")
        self._write({"t": "interrupt"})

    _frame = S.Owner._frame
    drain = S.Owner.drain

    def close(self):
        try:
            self._write({"t": "detach"})
        except OSError:
            pass
        try:
            self.s.close()
        except OSError:
            pass
        self.out.close()


def priced(rh, t0, t1=None):
    """M.priced_usage, split: `real_musd` prices only requests that reported usage; a 2xx stream with no usage (a stream
    the guard cut: the Root then charges its whole reservation, DV-3) is reported apart as `cut_root_musd`. The program
    accounting takes the larger of the priced sum and the balance drop, so a cut stream's true cost still counts once
    the balance settles (deviation D2, protocol §10.7)."""
    u = M.priced_usage(rh, t0, t1)
    cut = 0
    for r in S.jl(os.path.join(rh, "egress/usage/*.jsonl")):
        if r.get("t") not in ("end", "sealed") or isinstance(r.get("usage"), dict):
            continue
        t = M.parse_iso(r.get("at"))
        if t is None or t < t0 or (t1 and t > t1) or not 200 <= int(r.get("status") or 0) < 300:
            continue
        cut += int(r.get("cost_musd") or 0)
    u["cut_root_musd"] = cut
    u["real_musd"] = max(0, u["real_musd"] - cut)
    return u


def drive(a, o, line, guard, t0, logf):
    """Sends one owner line and waits for busy -> idle, applying the run guard (cap, wall, program floor).
    Returns {rc, stop, model_turns, wall_ms, reply_tail}; rc 0 ended by itself, 3 stopped by a guard, 4 infra."""
    o.drain(0.5)
    busy_seen = o.busy
    o.send(line)
    ts, texts, turns, stop, last_poll, last_bal = time.monotonic(), [], 0, None, 0.0, time.monotonic()
    started = False
    while True:
        now = time.monotonic()
        if now - last_poll >= 5:
            last_poll = now
            u = priced(a.rh, t0)
            bal = "skip"
            if a.mode == "real" and now - last_bal >= M.BAL_S:
                last_bal = now
                bal = M.read_balance(a.ih, "real")
            s, act = guard.tick(u["real_musd"], bal)
            logf.write(json.dumps({"t": iso(time.time()), "ev": "poll", "real_est_musd": u["real_musd"],
                                   "root_musd": u["root_musd"], "requests": u["requests"], "stop": s}) + "\n")
            logf.flush()
            if act == "freeze" or now - ts > a.wall_s:
                stop = s or "wall"
                break
            if not started and not busy_seen and now - ts > 90:
                return {"rc": 4, "stop": "never_busy", "model_turns": 0, "wall_ms": int((now - ts) * 1000), "reply_tail": ""}
        try:
            l = o.q.get(timeout=0.5)
        except Exception:
            continue
        if l is None:
            return {"rc": 4, "stop": "attach_closed", "model_turns": turns, "wall_ms": int((time.monotonic() - ts) * 1000),
                    "reply_tail": "".join(texts)[-1500:]}
        v = o._frame(l)
        f = v.get("frame") if isinstance(v.get("frame"), dict) else v   # as S.Owner.task reads frames
        k = f.get("kind") or (f.get("display") or {}).get("kind")
        text = f.get("text") if isinstance(f.get("text"), str) else (f.get("display") or {}).get("text", "")
        if v.get("t") == "display" or f.get("t") == "display":
            if k in ("text", None):
                texts.append(text or "")
            if k == "turn_end":
                turns += 1
        if v.get("t") == "state":
            if v.get("busy") is True:
                busy_seen = started = True
            elif v.get("busy") is False and busy_seen:
                return {"rc": 0, "stop": None, "model_turns": turns, "wall_ms": int((time.monotonic() - ts) * 1000),
                        "reply_tail": "".join(texts)[-1500:]}
    # a guard stop: freeze spend first (the Root refuses every further request), then interrupt and /stop
    frz = None
    if a.params:
        p = freeze_params(jload(a.params), a.rh)
        frz = apply_charter(a.rh, a.home, p)[0]
    try:
        o.interrupt()
        o.send("/stop")
    except OSError:
        pass
    end = time.monotonic() + 60
    while o.busy and time.monotonic() < end:
        o.drain(1)
    return {"rc": 3, "stop": stop, "freeze_rc": frz, "model_turns": turns, "wall_ms": int((time.monotonic() - ts) * 1000),
            "reply_tail": "".join(texts)[-1500:]}


def idle_window(a, o, guard, t0, logf, window_s, quiet_s):
    """Phase 1: keep the Root up after the turn so background turns and pending admissions can finish (protocol §3.2).
    Ends after quiet_s with no new zone request and no busy body, or at window_s, or at a guard stop."""
    end = time.monotonic() + window_s
    n_last, quiet_since = None, time.monotonic()
    while time.monotonic() < end:
        o.drain(5)
        u = priced(a.rh, t0)
        s, act = guard.tick(u["real_musd"], "skip")
        if act == "freeze":
            if a.params:
                apply_charter(a.rh, a.home, freeze_params(jload(a.params), a.rh))
            return {"stop": s, "s": int(window_s - (end - time.monotonic()))}
        if u["requests"] != n_last or o.busy:
            n_last, quiet_since = u["requests"], time.monotonic()
        elif time.monotonic() - quiet_since >= quiet_s:
            break
    logf.write(json.dumps({"t": iso(time.time()), "ev": "idle_end", "requests": n_last}) + "\n")
    return {"stop": None, "s": int(window_s - max(0, end - time.monotonic()))}


def wait_turn_close(rh, t0, timeout=90):
    """ADR-011 §3.1: the Root closes an owner turn after 30 s with no zone egress in flight, and only then writes the
    turn's usage line (caps used, with evidence). Stopping the Root at once would lose phase 2's Root evidence, so wait
    for a line that starts at or after t0 (only on a Root that keeps the ledger: ROOT_HOME/extend exists)."""
    if not os.path.isdir(os.path.join(rh, "extend")):
        return {"usage_line": None, "s": 0, "why": "no extend dir (pre-ADR-011 Root)"}
    t_start = time.time()
    while time.time() - t_start < timeout:
        for p in sorted(glob.glob(os.path.join(rh, "extend/usage/*.jsonl"))):
            for r in S.jl(p):
                st = parse_iso(r.get("start") or "")
                if st is not None and st >= t0 - 1:
                    return {"usage_line": True, "s": int(time.time() - t_start)}
        time.sleep(3)
    return {"usage_line": False, "s": int(time.time() - t_start)}


def run_one(a):
    """With the Root running (the shell started it). Writes <run>/result.json; rc as drive()."""
    spec = jload(a.spec)
    task = jload(os.path.join(a.task_dir, "task.json"))
    a.wall_s = spec["wall_s"]
    t0 = time.time()
    logf = open(os.path.join(a.run, "driver.jsonl"), "a")
    res = {"tag": spec["tag"], "t0": iso(t0), "spec": spec}
    ok, ws = set_workspace(a, a.ws)
    res["workspace"] = ws
    if not ok:
        res.update(rc=4, infra="workspace_not_applied")
        wj(os.path.join(a.run, "result.json"), res)
        return 4
    bal0 = M.read_balance(a.ih, "real") if a.mode == "real" else None
    res["balance_before_musd"] = bal0
    # bal0=None: the balance lags by minutes, so a drop since this run's start mixes in earlier runs' bills (a false
    # `real_cap:balance` stop on 2026-10-22, deviation D2); the run cap is priced, the balance checks the program floor
    guard = M.Guard(spec["cap_musd"], None, floor=a.floor_musd if a.floor_musd >= 0 else None)
    o = RawOwner(Ns(ih=a.ih, rh=a.rh, run=a.run, home=a.home))
    try:
        o.drain(3)
        if o.busy:  # a run resumed at boot (a fresh session should prevent it): interrupt it before the task
            o.interrupt()
            end = time.monotonic() + 60
            while o.busy and time.monotonic() < end:
                o.drain(1)
            res["quiesced"] = True
        line = attempt_line(task["prompt"], spec["phase"], spec["k"])
        res["line_sha256"] = hashlib.sha256(line.encode()).hexdigest()
        res["turn"] = drive(a, o, line, guard, t0, logf)
        if spec.get("idle_s") and res["turn"]["rc"] == 0:
            idle_s, quiet_s = spec["idle_s"], PREREG["p1_idle_quiet_s"]
            if a.mode == "mock" and os.environ.get("EVO_MOCK_FAST") == "1":  # the $0 dry run only; never real
                idle_s, quiet_s = min(idle_s, 30), 5
            res["idle"] = idle_window(a, o, guard, t0, logf, idle_s, quiet_s)
        o.drain(2)
        res["turn_close"] = wait_turn_close(a.rh, t0)
    finally:
        o.close()
    u = priced(a.rh, t0)
    res.update(rc=res["turn"]["rc"], real_est_musd=u["real_musd"], cut_root_musd=u["cut_root_musd"], root_musd=u["root_musd"], requests=u["requests"],
               tokens={"in": u["in"], "cache_read": u["cr"], "out": u["out"]}, t1=iso(time.time()),
               guard_stop=guard.stop, halt=guard.halt)
    if a.mode == "real":
        res["balance_after_musd"] = M.read_balance(a.ih, "real")
    if res["turn"]["rc"] == 4:
        res["infra"] = res["turn"]["stop"]
    wj(os.path.join(a.run, "result.json"), res)
    return 5 if guard.halt else res["turn"]["rc"]


# ============================================================================================ attribution


def attrib(rh, ih, since, until=None, seed_sha=None):
    """Capabilities created and used in [since, until]: ADR-011's Root records when they exist (evidence root/zone as the
    Root tags them), else zone-side traces (the body's commits and trailers, skills, the ext catalog), tagged `zone`."""
    out = {"sources": {}, "created": [], "used": {}, "records": {}}
    lin = [r for r in S.lineage(rh) if (parse_iso(r.get("ts") or r.get("at") or "") or 0) >= since
           and (until is None or (parse_iso(r.get("ts") or r.get("at") or "") or 0) <= until)]
    kinds = ("change_open", "change_close", "cap_state", "cap_use", "harm", "snapshot", "restore", "grant_request")
    for r in lin:
        if r.get("kind") in kinds:
            out["records"].setdefault(r["kind"], []).append(r.get("data"))
    out["sources"]["lineage_extend"] = bool(out["records"])
    for d in out["records"].get("change_open", []):
        for c in (d or {}).get("caps") or []:
            if c.get("op") in ("add", "modify"):
                out["created"].append({"id": c.get("id"), "ver": c.get("ver"), "op": c.get("op"),
                                       "evidence": c.get("evidence"), "scope": (d or {}).get("scope")})
    usage = sorted(glob.glob(os.path.join(rh, "extend/usage/*.jsonl")))
    out["sources"]["usage_ledger"] = bool(usage)
    for p in usage:
        for r in S.jl(p):
            t = parse_iso(r.get("start") or r.get("ts") or r.get("at") or "") or 0   # X5 usage lines carry `start`
            if t < since or (until and t > until):
                continue
            for c in r.get("caps") or []:
                cid = c.get("id") if isinstance(c, dict) else c
                ev = c.get("evidence", "?") if isinstance(c, dict) else "?"
                n = int(c.get("n") or c.get("uses") or 1) if isinstance(c, dict) else 1
                key = f"{cid}|{ev}"
                out["used"][key] = out["used"].get(key, 0) + n
    caps = jload(os.path.join(ih, "root/caps.json"))
    out["sources"]["caps_json"] = caps is not None
    if caps is not None:
        out["caps_json"] = caps
    # zone-side fallback (always collected; never counted as Root evidence)
    zone = {}
    git = ["/usr/bin/git", "-c", "core.hooksPath=/dev/null", "--git-dir", os.path.join(ih, "lineage/body.git")]
    env = {"PATH": "/usr/bin:/bin", "HOME": "/nonexistent", "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null"}
    rng = f"{seed_sha}..main" if seed_sha else "main"
    try:
        r = subprocess.run(git + ["log", "--format=%H%x09%ct%x09%s%x09%(trailers:key=Cap,valueonly,separator=%x2C)", rng],
                           capture_output=True, text=True, env=env, timeout=30)
        zone["commits"] = [dict(zip(("sha", "ct", "subject", "caps"), ln.split("\t", 3))) for ln in r.stdout.splitlines()
                           if ln.strip()]
    except (OSError, subprocess.SubprocessError):
        zone["commits"] = None
    sk = os.path.join(ih, "body/skills")
    zone["skills"] = sorted(os.listdir(sk)) if os.path.isdir(sk) else []
    cat = jload(os.path.join(ih, "state/ext/catalog.json"))
    zone["ext"] = sorted((cat or {}).get("extensions", {}) if isinstance((cat or {}).get("extensions"), dict)
                         else [e.get("name") for e in (cat or {}).get("extensions", []) if isinstance(e, dict)])
    out["zone"] = zone
    return out


# ============================================================================================ the control (protocol v3)
# Claude Code (the installed CLI) driving deepseek-flash through DeepSeek's Anthropic endpoint: the harness that HAS the
# targeted mechanisms (todo/task tracking re-surfaced while items stay open). Same task prompt, workspace, run cap and
# wall cap as INIT; isolation: a fresh CLAUDE_CONFIG_DIR and HOME, web tools disallowed, no MCP, a Seatbelt profile that
# lets it write only its temp workspace/config/tmp and read nothing of the owner's home or the dev checkout except those.

CONTROL_BIN = os.path.expanduser("~/.local/bin/claude")
CONTROL_URL = "https://api.deepseek.com/anthropic"
CONTROL_MODEL_VARS = ("ANTHROPIC_MODEL", "ANTHROPIC_SMALL_FAST_MODEL", "ANTHROPIC_DEFAULT_SONNET_MODEL",
                      "ANTHROPIC_DEFAULT_OPUS_MODEL", "ANTHROPIC_DEFAULT_HAIKU_MODEL", "CLAUDE_CODE_SUBAGENT_MODEL")
KEY_FILE = os.path.expanduser("~/.config/init-agent/deepseek.key")


def control_profile(ws, cfg, tmp, real_home):
    q = lambda p: os.path.realpath(p).replace("\\", "\\\\").replace('"', '\\"')  # noqa: E731
    install = os.path.realpath(os.path.dirname(os.path.realpath(CONTROL_BIN)))
    return ("(version 1)\n(allow default)\n"
            f'(deny file-read* (subpath "{q(real_home)}"))\n'
            f'(allow file-read* (subpath "{q(ws)}") (subpath "{q(cfg)}") (subpath "{q(tmp)}") (subpath "{install}")'
            f' (literal "{q(real_home)}") (literal "{q(os.path.dirname(real_home))}"))\n'
            "(deny file-write*)\n"
            f'(allow file-write* (subpath "{q(ws)}") (subpath "{q(cfg)}") (subpath "{q(tmp)}") (literal "/dev/null")'
            ' (literal "/dev/tty") (regex #"^/dev/fd/") (regex #"^/dev/ttys"))\n')


def anthropic_usage_musd(u, t):
    """DeepSeek list price (the window at t) of one Anthropic-shaped usage block, in µUSD (input_tokens priced in full)."""
    pi, pc, po = M.PRICE[M.window_at(t)]
    inp, cr = int(u.get("input_tokens") or 0), int(u.get("cache_read_input_tokens") or 0)
    cc, out = int(u.get("cache_creation_input_tokens") or 0), int(u.get("output_tokens") or 0)
    # DeepSeek's endpoint reports input_tokens INCLUDING the cache hits (smoke test 2026-10-22: 17,042 in, 16,896 cached)
    unc = inp - cr if inp >= cr else inp
    return -(-((unc + cc) * pi + cr * pc + out * po) // MUSD)


def est_output_tokens(m):
    """Stream-json assistant events carry the message's starting usage (output often 0): estimate the output from the
    content (text, thinking, tool input) at 3 characters per token, and keep the larger of the two."""
    n = 0
    for c in m.get("content") or []:
        if isinstance(c, dict):
            n += len(c.get("text") or "") + len(c.get("thinking") or "") + (len(json.dumps(c["input"])) if "input" in c else 0)
    return n // 3 + 1


def deepseek_balance(key):
    import urllib.request
    try:
        req = urllib.request.Request("https://api.deepseek.com/user/balance", headers={"Authorization": "Bearer " + key,
                                                                                     "Accept": "application/json"})
        with urllib.request.urlopen(req, timeout=20) as r:
            return M.parse_balance(json.loads(r.read().decode()))
    except Exception:
        return None


def control_run(a):
    """One control run. Writes <run>/control.jsonl (stream-json, key-scanned), <run>/result.json; rc 0 ended by itself,
    3 stopped by the cap or the wall, 4 infra, 5 program floor."""
    spec = jload(a.spec)
    task = jload(os.path.join(a.task_dir, "task.json"))
    key = open(KEY_FILE, "rb").read().decode().strip()   # into the child's environment only; never printed or written
    for d in (a.ws, a.cfg, a.tmp):
        os.makedirs(d, exist_ok=True)
    with open(os.path.join(a.cfg, "settings.json"), "w") as f:   # no hooks, no plugins, no telemetry; todo tools on
        json.dump({"env": {}, "permissions": {"deny": ["WebFetch", "WebSearch"]}}, f)
    t0 = time.time()
    res = {"tag": spec["tag"], "t0": iso(t0), "spec": spec, "harness": "claude-code " + subprocess.run(
        [CONTROL_BIN, "--version"], capture_output=True, text=True, timeout=30,
        env={"PATH": "/usr/bin:/bin", "HOME": a.cfg}).stdout.strip()}
    bal0 = deepseek_balance(key) if a.mode == "real" else None
    res["balance_before_musd"] = bal0
    if bal0 is not None and a.floor_musd >= 0 and bal0 <= a.floor_musd:
        res.update(rc=5, halt="program_cap:balance_floor")
        wj(os.path.join(a.run, "result.json"), res)
        return 5
    env = {"PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": a.cfg, "TMPDIR": a.tmp, "LANG": "en_US.UTF-8",
           "CLAUDE_CONFIG_DIR": a.cfg, "ANTHROPIC_BASE_URL": CONTROL_URL, "ANTHROPIC_AUTH_TOKEN": key,
           "API_TIMEOUT_MS": "600000", "CLAUDE_CODE_ENABLE_TODO_TOOLS": "1", "DISABLE_AUTOUPDATER": "1",
           "DISABLE_TELEMETRY": "1", "DISABLE_ERROR_REPORTING": "1", "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC": "1",
           "DISABLE_COST_WARNINGS": "1"}
    for v in CONTROL_MODEL_VARS:
        env[v] = PREREG["model"]
    prompt = task["prompt"]
    argv = ["/usr/bin/sandbox-exec", "-p", control_profile(a.ws, a.cfg, a.tmp, a.home), os.path.realpath(CONTROL_BIN), "-p", prompt,
            "--output-format", "stream-json", "--verbose", "--model", PREREG["model"], "--dangerously-skip-permissions",
            "--disallowedTools", "WebFetch", "WebSearch", "--strict-mcp-config", "--no-session-persistence"]
    out_path = os.path.join(a.run, "control.jsonl")
    usage, cost, stop, turns, last = {}, 0, None, 0, None
    p = subprocess.Popen(argv, cwd=a.ws, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                         stderr=open(os.path.join(a.run, "control.err"), "wb"), start_new_session=True)
    import threading
    import queue as _q
    q = _q.Queue()
    threading.Thread(target=lambda: [q.put(l) for l in p.stdout] + [q.put(None)], daemon=True).start()
    with open(out_path, "wb") as out:
        while True:
            if time.time() - t0 > spec["wall_s"]:
                stop = "wall"
                break
            try:
                line = q.get(timeout=1)
            except _q.Empty:
                continue
            if line is None:
                break
            out.write(line.replace(key.encode(), b"[REDACTED:key]"))
            try:
                ev = json.loads(line)
            except ValueError:
                continue
            if ev.get("type") == "assistant":
                m = ev.get("message") or {}
                if isinstance(m.get("usage"), dict) and m.get("id"):
                    u = dict(m["usage"])
                    prev = usage.get(m["id"], ({}, 0))[0]
                    est = int(prev.get("_est_out") or 0) + est_output_tokens(m)   # a message may stream in several events
                    u["_est_out"] = est
                    u["output_tokens"] = max(int(u.get("output_tokens") or 0), est)
                    usage[m["id"]] = (u, time.time())
                    turns = len(usage)
            if ev.get("type") == "result":
                last = {k: ev.get(k) for k in ("subtype", "is_error", "num_turns", "duration_ms", "usage")}
            cost = sum(anthropic_usage_musd(u, t) for u, t in usage.values())
            if cost >= spec["cap_musd"]:
                stop = "real_cap:priced_usage"
                break
    if p.poll() is None:   # our own child's process group (start_new_session): TERM, then KILL
        try:
            os.killpg(p.pid, signal.SIGTERM)
            p.wait(timeout=15)
        except (ProcessLookupError, subprocess.TimeoutExpired):
            try:
                os.killpg(p.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            p.wait()
    # the `result` event's usage is the last request's, not the session's (smoke test): the per-message sum stands
    tok = {"in": 0, "cache_read": 0, "out": 0}
    for u, _ in usage.values():
        tok["in"] += int(u.get("input_tokens") or 0) + int(u.get("cache_creation_input_tokens") or 0)
        tok["cache_read"] += int(u.get("cache_read_input_tokens") or 0)
        tok["out"] += int(u.get("output_tokens") or 0)
    leaks = 0
    for root, _, names in os.walk(a.ws):
        for n in names:
            pth = os.path.join(root, n)
            try:
                if not os.path.islink(pth) and key.encode() in open(pth, "rb").read():
                    leaks += 1
            except OSError:
                pass
    rc = 0 if stop is None else 3
    if not usage and stop is None:
        rc = 4   # nothing reached the model: a harness or sandbox failure, not the control's outcome
    res.update(rc=rc, guard_stop=stop, real_est_musd=cost, requests=len(usage), tokens=tok, infra="control_no_turn" if rc == 4 else None,
               result=last, t1=iso(time.time()), turn={"rc": rc, "model_turns": turns},
               key_in_workspace_files=leaks, exit=p.returncode)
    if a.mode == "real":
        res["balance_after_musd"] = deepseek_balance(key)
    del key
    wj(os.path.join(a.run, "result.json"), res)
    return res["rc"]


# ============================================================================================ results and report


def record(led, spec, result, score):
    r = dict(spec)
    r.update(seq=len(led["runs"]) + 1, rc=result.get("rc"), infra=result.get("infra"), guard_stop=result.get("guard_stop"),
             real_est_musd=result.get("real_est_musd", 0), root_musd=result.get("root_musd", 0),
             requests=result.get("requests", 0), tokens=result.get("tokens"), t0=result.get("t0"), t1=result.get("t1"),
             model_turns=(result.get("turn") or {}).get("model_turns"), score=score.get("score", 0) if score else 0,
             tamper=bool(score and score.get("tamper")), score_exit=score.get("exit") if score else None)
    r["cut_root_musd"] = result.get("cut_root_musd", 0)
    if (not r["infra"] and r.get("guard_stop") == "real_cap:balance" and r["real_est_musd"] < spec["cap_musd"]
            and r["score"] < PREREG["solved_permille"]):
        r["infra"] = "balance_lag_stop"   # D2: a harness fault, not the agent's outcome
    if r["infra"]:
        r["score"] = 0
    led["runs"][spec["tag"]] = r
    pr = led.setdefault("program", {})
    for k in ("balance_before_musd", "balance_after_musd"):
        v = result.get(k)
        if isinstance(v, int):
            pr.setdefault("bal0_musd", v)
            pr["bal_last_musd"] = v
    if pr.get("bal0_musd") and "floor_musd" not in pr:
        pr["floor_musd"] = pr["bal0_musd"] - PREREG["program_cap_real_musd"]
    if result.get("halt"):
        led["halt"] = result["halt"]
    return r


def solved(r):
    return r.get("score", 0) >= PREREG["solved_permille"] and not r.get("infra")


def evaluate(led, attrib_by_tag=None):
    """The pre-registered reading (protocol §5). attrib_by_tag: {tag: attrib()} for phase-1 and phase-2 runs."""
    attrib_by_tag = attrib_by_tag or {}
    sets = led.get("sets") or {"train": [], "heldout": []}
    created = {}
    for tag, at in attrib_by_tag.items():
        if tag.startswith("p1-"):
            for c in at.get("created", []):
                created[c["id"]] = c
    p1 = {t: [r.get("score", 0) for r in runs_of(led, 1, t, "evolved")] for t in sets["train"]}
    arms = {}
    for t in sets["heldout"]:
        arms[t] = {a: [r for r in runs_of(led, 2, t, a)] for a in ("evolved", "twin")}
    solved_runs = {a: sum(solved(r) for t in arms for r in arms[t][a]) for a in ("evolved", "twin")}
    attributed = []
    for t, d in arms.items():
        ev, tw = d["evolved"], d["twin"]
        if len(ev) == PREREG["p2_runs"] and all(solved(r) for r in ev) and not any(solved(r) for r in tw):
            ev_ok = PREREG["success"].get("evidence")
            used = all(any(k.split("|")[0] in created and (ev_ok is None or k.endswith("|" + ev_ok))
                           for k in (attrib_by_tag.get(r["tag"]) or {}).get("used", {})) for r in ev)
            attributed.append({"task": t, "phase1_caps_used_in_both": used, "evidence": PREREG["success"].get("evidence")})
    n_attr = sum(1 for x in attributed if x["phase1_caps_used_in_both"])
    s = PREREG["success"]
    evolved = len(created) >= s["min_new_caps"]
    helped = solved_runs["evolved"] - solved_runs["twin"] >= s["min_extra_solved_runs"]
    halt = led.get("halt") or ""
    verdict = ("stopped_in_phase0" if halt.startswith("p0") else
               "evolved_and_solved" if evolved and helped and n_attr >= s["min_attributed_tasks"] else
               "evolved_gain_unattributed" if evolved and helped else
               "evolved_no_heldout_gain" if evolved else "did_not_evolve")
    mean = {a: (sum(r.get("score", 0) for t in arms for r in arms[t][a]) // max(1, sum(len(arms[t][a]) for t in arms)))
            for a in ("evolved", "twin")}
    train_solved = sorted(t for t, sc in p1.items() if any(x >= PREREG["solved_permille"] for x in sc))
    return {"verdict": verdict, "success": verdict == s["verdict"], "created_caps": sorted(created), "phase1_scores": p1, "solved_runs": solved_runs,
            "mean_permille": mean, "attributed": attributed, "train_solved": train_solved,
            "training_only": bool(train_solved) and not helped,
            "spend": {f"p{k}": spent_phase(led, k) for k in (0, 1, 2)}, "program_spent_musd": spent_program(led)}


# ============================================================================================ readiness


def ready(bins):
    """Which ADR-011 interfaces the staged binaries and the seed carry (byte markers; the build lanes confirm)."""
    items = {}
    blobs = {}
    for name in ("init-root", "init-kernel"):
        p = os.path.join(bins, name)
        try:
            with open(p, "rb") as f:
                blobs[name] = f.read()
        except OSError:
            blobs[name] = None
    for k, (b, needle) in ADR011_MARKERS.items():
        items[k] = blobs.get(b) is not None and needle in blobs[b]
    items["seed.skill_extend"] = os.path.isfile(os.path.join(REPO, "seed", "skills", "extend", "SKILL.md"))
    items["root.workspace_set"] = blobs.get("init-root") is not None and b"workspace.set" in blobs["init-root"]
    items["root.memwatch"] = blobs.get("init-root") is not None and b"mem_cap" in blobs["init-root"]
    reg = [t for g in ("train", "heldout", "spares") for t in PREREG["tasks"][g]]
    items["tasks_missing"] = [t for t in reg if t not in P.task_ids()]
    adr = all(items[k] for k in list(ADR011_MARKERS) + ["seed.skill_extend"])
    return {"items": items, "adr011_ready": adr,
            "phase0_ready": items["root.workspace_set"] and not [t for t in items["tasks_missing"] if t not in PREREG["tasks"]["spares"]],
            "waiting_on_adr011": [k for k in list(ADR011_MARKERS) + ["seed.skill_extend"] if not items[k]]}


# ============================================================================================ mock


def mock_script(path, name):
    """The $0 dry run's stand-in agent: looks around, writes one note, ends (scores stay at the initial workspace)."""
    U = {"input": 6000, "cache_read": 0, "output": 30}
    meta = {"v": 1, "kind": "meta", "provider": "deepseek", "model": "deepseek-flash", "name": name, "chunk": 16}
    T = lambda r: {"v": 1, "kind": "turn", "reply": r}  # noqa: E731
    lines = [meta,
             T({"think": "Look around.", "calls": [{"name": "bash", "input": {"command": "ls -la; find . -maxdepth 2 -type f | head -40"}}],
                "stop": "tool_use", "usage": U}),
             T({"think": "Note.", "calls": [{"name": "write", "input": {"path": "NOTES.dryrun.txt", "content": "evo dry run\n"}}],
                "stop": "tool_use", "usage": U}),
             T({"text": "I could not finish this task.", "stop": "end_turn", "usage": U})]
    lines += [T({"text": "ok", "stop": "end_turn", "usage": U})] * 6
    with open(path, "w") as f:
        f.write("".join(json.dumps(l, sort_keys=True) + "\n" for l in lines))


# ============================================================================================ selftest


def selftest(with_tasks=True):
    fails = []
    PREREG = globals()["PREREG"]   # a local alias: the v2 section swaps the module's PREREG and back

    def ok(c, what):
        print(("ok   " if c else "FAIL ") + what)
        if not c:
            fails.append(what)

    # 1 the protocol's pre-registration blocks equal PREREG_V1 (record) and PREREG (v2, in force)
    try:
        txt = open(PROTOCOL, encoding="utf-8").read()
        m1 = re.search(r"<!-- PREREG BEGIN -->\s*```json\n(.*?)```\s*<!-- PREREG END -->", txt, re.S)
        ok(m1 is not None and json.loads(m1.group(1)) == PREREG_V1, "prereg_drift: protocol §7 JSON == PREREG_V1 (record)")
        m2 = re.search(r"<!-- PREREG v2 BEGIN -->\s*```json\n(.*?)```\s*<!-- PREREG v2 END -->", txt, re.S)
        ok(m2 is not None and json.loads(m2.group(1)) == PREREG_V2, "prereg_drift: protocol §10 JSON == PREREG_V2 (record)")
        m3 = re.search(r"<!-- PREREG v3 BEGIN -->\s*```json\n(.*?)```\s*<!-- PREREG v3 END -->", txt, re.S)
        ok(m3 is not None and json.loads(m3.group(1)) == PREREG_V3, "prereg_drift: protocol §11 JSON == PREREG_V3 (record)")
        m31 = re.search(r"<!-- PREREG v3.1 BEGIN -->\s*```json\n(.*?)```\s*<!-- PREREG v3.1 END -->", txt, re.S)
        ok(m31 is not None and json.loads(m31.group(1)) == PREREG_V31, "prereg_drift: protocol §11.6 JSON == PREREG_V31 (record)")
        m32 = re.search(r"<!-- PREREG v3.2 BEGIN -->\s*```json\n(.*?)```\s*<!-- PREREG v3.2 END -->", txt, re.S)
        ok(m32 is not None and json.loads(m32.group(1)) == PREREG, "prereg_drift: protocol §11.7 JSON == PREREG (v3.2)")
    except OSError:
        ok(False, "prereg_drift: measurements/evo-test-protocol.md missing")
    # 2 v2 schedule and classification on synthetic roles (1 training, 2 held-out, 1 spare), under PREREG_V2
    prereg_v3 = globals()["PREREG"]
    globals()["PREREG"] = json.loads(json.dumps(PREREG_V2))
    PREREG = globals()["PREREG"]
    saved = PREREG["tasks"]
    PREREG["tasks"] = {"train": ["tr-a"], "heldout": ["ho-a", "ho-b"], "spares": ["sp-a"]}
    try:
        idx = {"tasks": {t: {} for t in ("tr-a", "ho-a", "ho-b", "sp-a")}}
        led = ledger_new("mock")
        seen = []
        scores = {"ho-a": [1000, 400]}
        while True:
            sp = next_spec(idx, led, 0)
            if not sp or len(seen) > 20:
                break
            seen.append(sp["tag"])
            record(led, sp, {"rc": 0, "real_est_musd": 20_000}, {"score": scores.get(sp["task"], [100, 200])[sp["k"] - 1]})
        ok(seen == ["p0-tr-a-genesis-r1", "p0-ho-a-genesis-r1", "p0-ho-b-genesis-r1", "p0-tr-a-genesis-r2",
                    "p0-ho-b-genesis-r2", "p0-sp-a-genesis-r1", "p0-sp-a-genesis-r2"],
           f"p0: run 1 of all, run 2 of the unsolved, then the spare ({len(seen)})")
        c = classify(idx, led)
        ok(c and c["proceed"] and c["sets"] == {"train": ["tr-a"], "heldout": ["ho-b", "sp-a"]}, "classify: dropped -> spare")
        led4 = ledger_new("mock")
        for t in ("tr-a", "ho-a", "ho-b"):
            record(led4, spec_for(0, t, "genesis", 1, "g0", led4), {"rc": 0}, {"score": 1000 if t != "ho-b" else 300})
        ok(p0_early_stop(idx, led4) is not None, "p0 early stop: 2 solved on run 1, 1 spare -> stop")
        led4["runs"]["p0-tr-a-genesis-r1"]["score"] = 200
        ok(p0_early_stop(idx, led4) is None, "p0 early stop: 1 solved, 1 spare -> go on to run 2")
        led5 = ledger_new("mock")
        for t, sc in (("tr-a", 1000), ("ho-a", 300), ("ho-b", 300)):
            record(led5, spec_for(0, t, "genesis", 1, "g0", led5), {"rc": 0}, {"score": sc})
        for t in ("ho-a", "ho-b"):
            record(led5, spec_for(0, t, "genesis", 2, "g0", led5), {"rc": 0}, {"score": 300})
        for k in (1, 2):
            record(led5, spec_for(0, "sp-a", "genesis", k, "g0", led5), {"rc": 0}, {"score": 1000 if k == 2 else 100})
        c5 = classify(idx, led5)
        ok(c5 and not c5["proceed"], "classify: the spare is solved too -> stop (genesis solves the set)")
        # an infra failure is rerun once, then counted
        led2 = ledger_new("mock")
        sp = next_spec(idx, led2, 0)
        record(led2, sp, {"rc": 4, "infra": "never_busy"}, None)
        sp2 = next_spec(idx, led2, 0)
        ok(sp2["base_tag"] == sp["base_tag"] and sp2["tag"].endswith("-x1"), "infra failure: one rerun with a new tag")
        record(led2, sp2, {"rc": 4, "infra": "never_busy"}, None)
        ok(next_spec(idx, led2, 0)["base_tag"] != sp["base_tag"], "infra reruns exhausted: the schedule moves on")
        # phase 1: at most 3 attempts, stop at a solve; first attempt restores g0, then keep
        led["sets"] = c["sets"]
        tags = []
        while True:
            sp = next_spec(idx, led, 1)
            if not sp or len(tags) > 5:
                break
            tags.append((sp["tag"], sp["restore"]))
            led["p1"]["started"] = True
            record(led, sp, {"rc": 0, "real_est_musd": 30_000}, {"score": 1000 if sp["k"] == 2 else 300})
        ok([r for _, r in tags] == ["g0", "keep"], "p1: g0 once, then the same home; stop at the first 1000")
        led6 = ledger_new("mock")
        led6["sets"] = c["sets"]
        n6 = 0
        while next_spec(idx, led6, 1) and n6 < 6:
            record(led6, next_spec(idx, led6, 1), {"rc": 0}, {"score": 300})
            n6 += 1
        ok(n6 == 3, "p1: at most 3 attempts")
        ok(attempt_line("Do X.", 1, 2).startswith(PREREG["retry_preface"]) and attempt_line("Do X.", 1, 1) == "Do X."
           and attempt_line("Do X.", 2, 2) == "Do X.", "retry preface only on phase-1 retries")
        # phase 2: 2 tasks x 2 runs x 2 arms, interleaved ABBA, restores e1 / g0
        led["p1"]["e1"] = True
        order = []
        while True:
            sp = next_spec(idx, led, 2)
            if not sp or len(order) > 10:
                break
            order.append((sp["task"], sp["arm"], sp["k"], sp["restore"]))
            record(led, sp, {"rc": 0, "real_est_musd": 25_000}, {"score": 1000 if (sp["arm"] == "evolved" and sp["task"] == "ho-b") else 200})
        ok(len(order) == 8 and all((r == "e1") == (a == "evolved") for _, a, _, r in order), "p2: 2 x 2 x 2, arm snapshots")
        first = {}
        for t, a, k, _ in order:
            first.setdefault((t, k), a)
        ok(all(first[(t, 1)] != first[(t, 2)] for t in c["sets"]["heldout"]), "p2: ABBA (arm order flips between runs)")
        # evaluation: the strict verdict needs Root evidence
        at = {r["tag"]: {"created": [{"id": "skill/x"}]} for r in led["runs"].values() if r["phase"] == 1}
        for r in led["runs"].values():
            if r["phase"] == 2:
                at[r["tag"]] = {"used": {"skill/x|root": 2}} if r["arm"] == "evolved" else {"used": {}}
        v = evaluate(led, at)
        ok(v["verdict"] == "evolved_and_solved" and v["success"] and v["solved_runs"] == {"evolved": 2, "twin": 0},
           f"evaluate: {v['verdict']} (2/2 vs 0/2 with Root evidence)")
        for r in led["runs"].values():
            if r["phase"] == 2 and r["arm"] == "evolved":
                at[r["tag"]] = {"used": {"skill/x|zone": 2}}
        v3 = evaluate(led, at)
        ok(v3["verdict"] == "evolved_gain_unattributed" and not v3["success"], "evaluate: zone evidence only -> not success")
        ok(evaluate(led, {})["verdict"] == "did_not_evolve", "evaluate: no created caps -> did_not_evolve")
        # 3 money gates ($1.00 program)
        led3 = ledger_new("real")
        sp = spec_for(0, "tr-a", "genesis", 1, "g0", led3)
        ok(budget_gate(led3, sp)[0], "budget: a fresh program may start")
        led3["runs"]["x"] = {"phase": 0, "real_est_musd": 310_000}
        ok(not budget_gate(led3, sp)[0], "budget: phase 0 envelope stops phase 0")
        led3["runs"]["x"] = {"phase": 1, "real_est_musd": 600_000}
        sp1 = spec_for(1, "tr-a", "evolved", 1, "g0", led3)
        ok(not budget_gate(led3, sp1)[0], "budget: phase 2's envelope stays reserved during phase 1")
        sp2 = spec_for(2, "ho-a", "twin", 1, "g0", led3)
        ok(budget_gate(led3, sp2)[0], "budget: phase 2 may use its reservation")
        led3["program"] = {"bal0_musd": 6_890_000, "bal_last_musd": 5_920_000}
        ok(not budget_gate(led3, sp2)[0], "budget: the balance drop counts when larger than the priced sum")
    finally:
        PREREG["tasks"] = saved
        globals()["PREREG"] = prereg_v3
    PREREG = prereg_v3
    # 2b v3 calibration: genesis 2/2 failures, then the control until its first solve; first kept tasks in order
    saved = PREREG["tasks"]
    PREREG["tasks"] = {"train": ["tr-a", "tr-b"], "heldout": ["ho-a", "ho-b", "ho-c"], "spares": []}
    try:
        idx = {"tasks": {t: {} for t in ("tr-a", "tr-b", "ho-a", "ho-b", "ho-c")}}
        led = ledger_new("mock")
        gen = {"tr-b": [1000]}
        while True:
            sp = next_spec(idx, led, 0)
            if not sp:
                break
            record(led, sp, {"rc": 0}, {"score": gen.get(sp["task"], [300, 300])[sp["k"] - 1]})
        ok(p0_status(led, "tr-b") == "dropped" and all(p0_status(led, t) == "beyond" for t in ("tr-a", "ho-a", "ho-b", "ho-c")),
           "v3 genesis: 2/2 failures are beyond, a solve drops")
        ctl = {"tr-a": [1000], "ho-a": [500, 600], "ho-b": [400, 1000], "ho-c": [1000]}
        seq = []
        while True:
            sp = next_control(idx, led)
            if not sp or len(seq) > 10:
                break
            seq.append(sp["tag"])
            record(led, sp, {"rc": 0}, {"score": ctl[sp["task"]][sp["k"] - 1]})
        ok(seq == ["p0-tr-a-control-r1", "p0-ho-a-control-r1", "p0-ho-a-control-r2", "p0-ho-b-control-r1",
                   "p0-ho-b-control-r2", "p0-ho-c-control-r1"], f"v3 control: only on genesis failures, stop at a solve ({len(seq)})")
        ok(p0_status(led, "tr-a") == "beyond", "v3: control runs do not count as genesis runs")
        c = classify(idx, led)
        ok(c and c["proceed"] and c["sets"] == {"train": ["tr-a"], "heldout": ["ho-b", "ho-c"]}, "v3 classify: (a) and (b), first kept")
    finally:
        PREREG["tasks"] = saved
    g = M.Guard(40_000, 6_890_000, floor=5_890_000)
    ok(g.tick(39_000, "skip") == (None, None) and g.tick(40_000, "skip")[1] == "freeze", "guard: run cap")
    g = M.Guard(40_000, 6_890_000, floor=5_890_000)
    ok(g.tick(1_000, 5_880_000)[1] == "freeze", "guard: program floor by balance")
    # 4 config
    p = params("mock", "/nonexistent", REPO, "m", 50_000, 9, "/tc", "/dev", "gated")
    t = charter_text(p)
    ok("read_home = false" in t and "fixtures" in t and "REAL-M7" not in t and "[extend]" not in t, "charter: read_home off, deny list, neutral purpose")
    ok('[extend]\nmode = "free"' in charter_text(dict(p, extend="free")), "charter: [extend] mode free when ADR-011 is ready")
    ok('task_usd = "0.20"' in kconfig_text(p), "kconfig: task backstop above cap + reservation")
    # 5 redaction and mock script
    b, n = M.redact_bytes(b"x person@example.org sk-ABCDEFGHIJKLMNOP agent@init.local", [])
    ok(b"person@example.org" not in b and b"sk-ABC" not in b and b"agent@init.local" in b and n == 2, "redaction")
    sc = tempfile.mkdtemp(prefix="evo-selftest-")
    try:
        mock_script(os.path.join(sc, "m.jsonl"), "evo-x")
        ls = [json.loads(x) for x in open(os.path.join(sc, "m.jsonl"))]
        ok(ls[0]["kind"] == "meta" and ls[-1]["reply"]["stop"] == "end_turn", "mock script")
        # 6 the sandboxed scorer: a synthetic task whose check tries to write outside its scratch dir
        td = os.path.join(sc, "task")
        os.makedirs(os.path.join(td, "check", "hidden"))
        with open(os.path.join(td, "check", "run"), "w") as f:
            f.write(P.RUN_SH)
        os.chmod(os.path.join(td, "check", "run"), 0o755)
        shutil.copyfile(os.path.join(P.TOOLS_DIR, "poolcheck.py"), os.path.join(td, "check", "poolcheck.py"))
        probe = os.path.join(sc, "escaped")
        with open(os.path.join(td, "check", "hidden", "c.sh"), "w") as f:
            f.write(f'echo PASS ran\nif touch "{probe}" 2>/dev/null; then echo PASS escaped; else echo FAIL escaped; fi\n')
        with open(os.path.join(td, "check", "check.json"), "wb") as f:
            f.write(P.dumps({"v": 1, "runner": "poolcheck/1", "scoring": "fraction",
                             "steps": [{"kind": "sh", "script": "hidden/c.sh", "names": ["ran", "escaped"]}]}))
        with open(os.path.join(td, "check", "protected.json"), "wb") as f:
            f.write(P.dumps([]))
        with open(os.path.join(td, "task.json"), "w") as f:
            json.dump({"id": "selftest", "check": {"argv": ["check/run"], "timeout_s": 60}}, f)
        os.makedirs(os.path.join(sc, "ws"))
        r = score_ws(td, os.path.join(sc, "ws"), sc)
        ok(r["score"] == 500 and not os.path.exists(probe), f"sandboxed scorer: writes outside scratch are denied ({r['score']})")
        # 7 the real task set, if present: every reference scores 1000 sandboxed, no solution.tar in the output
        reg = [t for g in ("train", "heldout", "spares") for t in PREREG["tasks"][g] if t in P.task_ids()]
        if with_tasks and reg:
            out = os.path.join(sc, "tasks")
            ix = build_tasks(out, reg)
            ok(all(v["reference"] == 1000 for v in ix["tasks"].values()), f"build-tasks: {len(ix['tasks'])} registered references 1000 sandboxed")
            ok(not any("solution.tar" in fn for _, _, fn in os.walk(out)), "build-tasks: no solution.tar in the built set")
            ok(all(v["initial"] <= 500 for v in ix["tasks"].values()), "build-tasks: initial workspaces <= 500")
    finally:
        rm_tree(sc)
    print(f"{'PASS' if not fails else 'FAIL'}: {len(fails)} failing")
    return 0 if not fails else 1


# ============================================================================================ CLI


def main():
    ap = argparse.ArgumentParser(prog="evo_checks.py")
    sub = ap.add_subparsers(dest="cmd", required=True)
    p = sub.add_parser("selftest"); p.add_argument("--no-tasks", action="store_true")
    p = sub.add_parser("ready"); p.add_argument("--bins", default=os.path.join(REPO, "target", "evo-bin")); p.add_argument("--json", action="store_true")
    p = sub.add_parser("build-tasks"); p.add_argument("--out", required=True); p.add_argument("--only", default="")
    p = sub.add_parser("prep-ws"); p.add_argument("--task-dir", required=True); p.add_argument("--ws", required=True)
    p = sub.add_parser("next"); p.add_argument("--ledger", required=True); p.add_argument("--tasks", required=True); p.add_argument("--phase", type=int, required=True); p.add_argument("--out", required=True); p.add_argument("--mode")
    p = sub.add_parser("can-start"); p.add_argument("--ledger", required=True); p.add_argument("--spec", required=True); p.add_argument("--mode", required=True); p.add_argument("--no-disk", action="store_true")
    for name in ("params", "charter", "kconfig"):
        p = sub.add_parser(name)
        p.add_argument("--mode"); p.add_argument("--rh"); p.add_argument("--x"); p.add_argument("--cap-musd", type=int)
        p.add_argument("--mock-port", type=int, default=None); p.add_argument("--toolchain", default=""); p.add_argument("--devdir", default="")
        p.add_argument("--extend", default="gated"); p.add_argument("--params"); p.add_argument("--out", required=True)
    p = sub.add_parser("run-one")
    for k in ("--ih", "--rh", "--home", "--run", "--mode", "--ws", "--task-dir", "--spec", "--params"):
        p.add_argument(k, required=True)
    p.add_argument("--floor-musd", type=int, default=-1)
    p = sub.add_parser("score"); p.add_argument("--task-dir", required=True); p.add_argument("--ws", required=True); p.add_argument("--out", required=True)
    p = sub.add_parser("attrib"); p.add_argument("--rh", required=True); p.add_argument("--ih", required=True); p.add_argument("--since", required=True); p.add_argument("--until"); p.add_argument("--seed-sha"); p.add_argument("--out", required=True)
    p = sub.add_parser("record"); p.add_argument("--ledger", required=True); p.add_argument("--spec", required=True); p.add_argument("--result", required=True); p.add_argument("--score", required=True)
    p = sub.add_parser("classify"); p.add_argument("--ledger", required=True); p.add_argument("--tasks", required=True)
    p = sub.add_parser("mark"); p.add_argument("--ledger", required=True); p.add_argument("--key", required=True)
    p = sub.add_parser("report"); p.add_argument("--ledger", required=True); p.add_argument("--attrib-dir", required=True); p.add_argument("--out", required=True)
    p = sub.add_parser("freeze"); p.add_argument("--rh", required=True); p.add_argument("--home", required=True); p.add_argument("--params", required=True)
    p = sub.add_parser("redact"); p.add_argument("dir")
    p = sub.add_parser("mock-script"); p.add_argument("--out", required=True); p.add_argument("--name", required=True)
    sub.add_parser("roles")
    p = sub.add_parser("next-control"); p.add_argument("--ledger", required=True); p.add_argument("--tasks", required=True); p.add_argument("--out", required=True)
    p = sub.add_parser("control-run")
    for k in ("--run", "--mode", "--ws", "--cfg", "--tmp", "--task-dir", "--spec", "--home"):
        p.add_argument(k, required=True)
    p.add_argument("--floor-musd", type=int, default=-1)
    p = sub.add_parser("note"); p.add_argument("--ledger", required=True); p.add_argument("--text", required=True)
    a = ap.parse_args()

    if a.cmd == "selftest":
        return selftest(not a.no_tasks)
    if a.cmd == "ready":
        r = ready(a.bins)
        print(json.dumps(r, sort_keys=True, indent=None if a.json else 1))
        return 0
    if a.cmd == "build-tasks":
        ix = build_tasks(a.out, [x for x in a.only.split(",") if x] or None)
        print(json.dumps({k: {"group": v["group"], "kind": v["kind"], "initial": v["initial"]} for k, v in ix["tasks"].items()}, sort_keys=True))
        return 0
    if a.cmd == "prep-ws":
        rm_tree(a.ws)
        with open(os.path.join(a.task_dir, "workspace.tar"), "rb") as f:
            P.extract_tar(f.read(), a.ws)
        return 0
    if a.cmd == "next":
        led = jload(a.ledger)
        if led is None:
            led = ledger_new(a.mode or "?")
            wj(a.ledger, led)
        idx = jload(os.path.join(a.tasks, "index.json"))
        missing = [t for g in ("train", "heldout") for t in PREREG["tasks"][g] if t not in idx["tasks"]]
        if missing:
            print(f"registered tasks missing from the built set: {missing} (build them first: evo_test.sh <mode> tasks)", file=sys.stderr)
            return 3
        why = p0_early_stop(idx, led) if a.phase == 0 else None
        if why and not led.get("halt"):
            led["halt"] = "p0_early_stop: " + why + ": the set is too easy for genesis"
            wj(a.ledger, led)
            print(led["halt"], file=sys.stderr)
            return 0
        sp = next_spec(idx, led, a.phase)
        if sp:
            wj(a.out, sp)
            print(sp["tag"])
        return 0
    if a.cmd == "can-start":
        led, sp = jload(a.ledger), jload(a.spec)
        ok_, why = budget_gate(led, sp)
        if not ok_:
            print(why)
            return 5
        need = PREREG["start_min_free_mb"][f"p{sp['phase']}"]
        if not a.no_disk and free_mb() < need:
            print(f"disk: {free_mb()} MB free < {need} MB")
            return 7
        if a.mode == "real" and not offpeak_ok(time.time(), sp["wall_s"]):
            print("peak window inside the run's wall")
            return 6
        print("ok")
        return 0
    if a.cmd in ("params", "charter", "kconfig"):
        if a.cmd == "params":
            wj(a.out, params(a.mode, a.rh, REPO, a.x, a.cap_musd, a.mock_port, a.toolchain, a.devdir, a.extend))
            return 0
        p = jload(a.params)
        with open(a.out, "w") as f:
            f.write(charter_text(p) if a.cmd == "charter" else kconfig_text(p))
        return 0
    if a.cmd == "run-one":
        return run_one(a)
    if a.cmd == "score":
        sc = tempfile.mkdtemp(prefix="evo-score-")
        try:
            r = score_ws(a.task_dir, a.ws, sc)
        finally:
            rm_tree(sc)
        wj(a.out, r)
        print(json.dumps({"score": r["score"], "tamper": r["tamper"], "wall_ms": r["wall_ms"]}))
        return 0
    if a.cmd == "attrib":
        since = parse_iso(a.since)
        until = parse_iso(a.until) if a.until else None
        wj(a.out, attrib(a.rh, a.ih, since, until, a.seed_sha))
        return 0
    if a.cmd == "record":
        led = jload(a.ledger)
        r = record(led, jload(a.spec), jload(a.result, {}), jload(a.score, None))
        wj(a.ledger, led)
        print(json.dumps({"tag": r["tag"], "score": r["score"], "real_est_musd": r["real_est_musd"], "infra": r["infra"]}))
        return 0
    if a.cmd == "classify":
        led = jload(a.ledger)
        c = classify(jload(os.path.join(a.tasks, "index.json")), led)
        if c is None:
            print("pending")
            return 1
        led["p0"], led["sets"] = c, (c["sets"] if c["proceed"] else None)
        if not c["proceed"]:
            led["halt"] = "p0:" + c["why"]
        wj(a.ledger, led)
        print(json.dumps(c, sort_keys=True))
        return 0 if c["proceed"] else 5
    if a.cmd == "mark":
        led = jload(a.ledger)
        sec, k = a.key.split(".", 1)
        led.setdefault(sec, {})[k] = True
        wj(a.ledger, led)
        return 0
    if a.cmd == "report":
        led = jload(a.ledger)
        ab = {os.path.basename(p)[:-5]: jload(p, {}) for p in glob.glob(os.path.join(a.attrib_dir, "*.json"))}
        rep = {"prereg_sha256": prereg_sha(), "ledger_prereg_sha256": led.get("prereg_sha256"), "p0": led.get("p0"),
               "sets": led.get("sets"), "evaluation": evaluate(led, ab), "halt": led.get("halt"),
               "runs": [{k: r.get(k) for k in ("tag", "phase", "task", "arm", "k", "score", "real_est_musd", "requests",
                                                 "tokens", "model_turns", "guard_stop", "infra")}
                        for r in sorted(led["runs"].values(), key=lambda r: r.get("seq", 0))]}
        wj(a.out, rep)
        print(json.dumps(rep["evaluation"], sort_keys=True))
        return 0
    if a.cmd == "freeze":
        p = freeze_params(jload(a.params), a.rh)
        rc, err = apply_charter(a.rh, a.home, p)
        print(json.dumps({"rc": rc, "ceiling_musd": p["ceiling_musd"], "err": err[-200:]}))
        return 0 if rc == 0 else 1
    if a.cmd == "redact":
        print(json.dumps(M.redact_tree(a.dir)))
        return 0
    if a.cmd == "next-control":
        led, idx = jload(a.ledger), jload(os.path.join(a.tasks, "index.json"))
        sp = next_control(idx, led)
        if sp:
            wj(a.out, sp)
            print(sp["tag"])
        return 0
    if a.cmd == "control-run":
        return control_run(a)
    if a.cmd == "note":
        led = jload(a.ledger)
        led.setdefault("notes", []).append({"t": iso(time.time()), "text": a.text})
        wj(a.ledger, led)
        return 0
    if a.cmd == "roles":   # the registered tasks that exist (the spare may still be in authoring)
        print(",".join(t for g in ("train", "heldout", "spares") for t in PREREG["tasks"][g] if t in P.task_ids()))
        return 0
    if a.cmd == "mock-script":
        mock_script(a.out, a.name)
        return 0
    return 2


if __name__ == "__main__":
    sys.exit(main())
