#!/usr/bin/env python3
"""pool: build, validate and import the visible maintainer pool v1 (docs/m7-design.md §3.3; PROOF-LEAN D-L4).

Python >= 3.9, stdlib only. Run it with /usr/bin/python3 -I from the repo root:

    pool.py build    --out DIR [--only ID,...]                build task directories (kernel/gate.md §3.7.1)
    pool.py validate [--only ID,...] [--repeat 3] [--json]   determinism, schema, sources, ref-pass, naive-fail
    pool.py import   --home INIT_HOME [--replace]            validate, freeze into vault/gate/pool/<id>/ and
                                                             mirror read-only at kernel/pool/<id>/
    pool.py manifest [--check]                               write (or verify) pool.json from the task sources
    pool.py cases    [--only ID,...]                         regenerate hidden/cases.json of py_cases tasks

Common options: --toolchain BIN (the dir holding cargo/rustc for the checks; default: the sysroot of the rustc
on PATH), --cargo-target-dir DIR (shared CARGO_TARGET_DIR for Rust checks; default: a fresh dir per check).

Task sources live in tasks/<id>/:
  task.in.json   {id, title, family: e|c|t, difficulty 1-5, labels[], max_turns?, prompt, source{...},
                  protected[] (workspace paths the check verifies), steps[] (poolcheck steps; the build adds
                  the names of py_unittest, cargo_test and py_cases steps), scoring?}
  workspace/     the initial workspace, copied verbatim (a file's x bit gives mode 0755, else 0644; a file named
                 dot.gitignore is written as .gitignore, so the INIT repo never applies a task's ignore rules)
  workspace.gen  optional executable, run as `workspace.gen <ws>` after the copy (e.g. to build a git repo);
                 it runs with a clean environment: fixed git identity and dates, no global or system git config
  solution/      overlay giving a passing workspace, and/or solve.sh (run by sh with cwd = a copy of the
                 built workspace, same clean environment)
  hidden/        check data: copied to check/hidden/
  NOTICE         required for derived tasks; copied next to task.json and into the workspace

A built task directory: task.json (v3), workspace.tar, solution.tar, check/{run, poolcheck.py, check.json,
protected.json, hidden/...}, NOTICE. Tars are deterministic (sorted, mtime fixed, uid/gid 0, modes 0644/0755).
"""
from __future__ import annotations

import argparse
import ast
import datetime
import hashlib
import io
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import time

POOL_DIR = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
TASKS_DIR = os.path.join(POOL_DIR, "tasks")
TOOLS_DIR = os.path.join(POOL_DIR, "tools")
REPO = os.path.dirname(os.path.dirname(POOL_DIR))
POOL_ID = "pool-v1"
CREATED = "2026-10-20T00:00:00Z"
MTIME = 1792454400  # 2026-10-20T00:00:00Z
FAMILIES = {"e": 8, "c": 8, "t": 10}  # default max_turns per family (m7-design §3.1.3)
LIMITS = {"wall_s": 600, "max_cost_musd": 100_000, "max_output_tokens": 24_000, "max_input_tokens": 400_000}
CHECK_TIMEOUT_S = 60
CHECK_WALL_MAX_S = 10.0  # m7-design §3.1.1: tests <= 10 s on an M2
ID_RE = re.compile(r"^[a-z0-9][a-z0-9._-]{0,63}$")
LABEL_RE = re.compile(r"^[a-z0-9-]{1,40}$")
RUN_SH = """#!/bin/sh
# pool-v1 check entry point (fixtures/pool-v1/tools/poolcheck.py). cwd = the workspace; last stdout line = score.
d=$(cd "$(dirname "$0")" && pwd -P) || { echo '{"score_permille":0}'; exit 2; }
for py in python3 /usr/bin/python3; do
  if command -v "$py" >/dev/null 2>&1 &&
     "$py" -I -S -c 'import sys; sys.exit(0 if sys.version_info >= (3, 9) else 1)' >/dev/null 2>&1; then
    exec "$py" -I -S -B "$d/poolcheck.py" "$d"
  fi
done
echo "poolcheck: toolchain_missing: python3>=3.9" >&2
echo '{"score_permille":0}'
exit 4
"""

# Disjointness (m7-design §3.1.1, pack lint M7-P08). Sealed E lanes own Exercism slugs a-m; the benchmark slice
# (§3.6 B) is Aider-AI/polyglot-benchmark's list (names read at 7e0611e7, 2026-10-20); S1-C's CRUXEval samples
# (S1-C, 2026-10-20) plus sample_0 (S1-CORE's example template).
POLYGLOT = frozenset("""
affine-cipher beer-song book-store bottle-song bowling connect dominoes dot-dsl food-chain forth go-counting
grade-school grep hangman list-ops paasio phone-number pig-latin poker pov proverb react rest-api robot-name
scale-generator sgf-parsing simple-linked-list transpose tree-building two-bucket variable-length-quantity wordy
zebra-puzzle zipper accumulate acronym alphametics decimal doubly-linked-list fizzy gigasecond luhn-from macros
nucleotide-codons ocr-numbers parallel-letter-frequency say simple-cipher word-count xorcism
""".split())
CRUX_SEALED = frozenset([0, 5, 76, 183, 223, 363, 369, 399, 601, 623, 667, 729, 771, 780, 793])
SOURCES = {
    "exercism/python": ("6105f6d9d3ef66ce3d5c8f529d2882af3d76b706", "LICENSES/Exercism-MIT.txt"),
    "exercism/rust": ("cc099ae090a6acbe6f53f1e36325263046ba333e", "LICENSES/Exercism-MIT.txt"),
    "facebookresearch/cruxeval": ("190faf16d175b5847b0af05d937872b1fb395942", "LICENSES/CRUXEval-MIT.txt"),
}


class PoolError(Exception):
    pass


def say(msg: str) -> None:
    sys.stderr.write(msg + "\n")


def dumps(obj) -> bytes:
    return (json.dumps(obj, sort_keys=True, indent=1, ensure_ascii=True) + "\n").encode("ascii")


def sha256(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


def task_ids() -> list:
    return sorted(d for d in os.listdir(TASKS_DIR) if os.path.isfile(os.path.join(TASKS_DIR, d, "task.in.json")))


def load_src(tid: str) -> dict:
    with open(os.path.join(TASKS_DIR, tid, "task.in.json"), "rb") as f:
        t = json.loads(f.read().decode("utf-8"))
    if t.get("id") != tid:
        raise PoolError("%s: task.in.json id %r differs from the directory name" % (tid, t.get("id")))
    return t


# ------------------------------------------------------------------------------------------------ build


def clean_env(tmp: str) -> dict:
    return {
        "PATH": "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": tmp, "TMPDIR": tmp, "LANG": "C", "LC_ALL": "C", "TZ": "UTC",
        "GIT_CONFIG_NOSYSTEM": "1", "GIT_CONFIG_GLOBAL": "/dev/null", "GIT_TERMINAL_PROMPT": "0",
        "GIT_AUTHOR_NAME": "Pool Fixture", "GIT_AUTHOR_EMAIL": "pool@init.local",
        "GIT_COMMITTER_NAME": "Pool Fixture", "GIT_COMMITTER_EMAIL": "pool@init.local",
        "GIT_AUTHOR_DATE": "2026-10-01T12:00:00+00:00", "GIT_COMMITTER_DATE": "2026-10-01T12:00:00+00:00",
        "PYTHONDONTWRITEBYTECODE": "1", "PYTHONHASHSEED": "0",
    }


def copy_tree(src: str, dst: str) -> None:
    for dirpath, dirnames, filenames in os.walk(src):
        dirnames[:] = sorted(d for d in dirnames if d not in ("__pycache__", ".DS_Store"))
        rel = os.path.relpath(dirpath, src)
        out = os.path.join(dst, rel) if rel != "." else dst
        os.makedirs(out, exist_ok=True)
        for name in sorted(filenames):
            if name == ".DS_Store" or name.endswith(".pyc"):
                continue
            s = os.path.join(dirpath, name)
            if os.path.islink(s):
                raise PoolError("symlink in task source: %s" % s)
            # a source file named dot.gitignore becomes .gitignore: a real .gitignore inside fixtures/ would
            # make the INIT repo itself ignore the task's files
            d = os.path.join(out, ".gitignore" if name == "dot.gitignore" else name)
            if os.path.lexists(d):
                os.chmod(d, 0o644)
                os.unlink(d)
            shutil.copyfile(s, d)
            os.chmod(d, 0o755 if os.stat(s).st_mode & 0o111 else 0o644)


def run_hook(argv: list, cwd: str, what: str) -> None:
    tmp = tempfile.mkdtemp(prefix="pool-hook-")
    try:
        p = subprocess.run(argv, cwd=cwd, env=clean_env(tmp), stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                           stderr=subprocess.STDOUT, timeout=120)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    if p.returncode != 0:
        raise PoolError("%s failed (exit %d): %s" % (what, p.returncode, p.stdout.decode("utf-8", "replace")[-2000:]))


def tree_files(root: str) -> list:
    """Sorted (rel, kind, mode) for every entry under root; links are refused."""
    out = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames.sort()
        for name in dirnames + sorted(filenames):
            p = os.path.join(dirpath, name)
            rel = os.path.relpath(p, root).replace(os.sep, "/")
            st = os.lstat(p)
            if stat.S_ISLNK(st.st_mode):
                raise PoolError("link in workspace: %s" % rel)
            if stat.S_ISDIR(st.st_mode):
                out.append((rel, "d", 0o755))
            elif stat.S_ISREG(st.st_mode):
                out.append((rel, "f", 0o755 if st.st_mode & 0o111 else 0o644))
            else:
                raise PoolError("special file in workspace: %s" % rel)
    return sorted(out)


def make_tar(root: str) -> bytes:
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w", format=tarfile.PAX_FORMAT) as tf:
        for rel, kind, mode in tree_files(root):
            ti = tarfile.TarInfo(rel)
            ti.mtime, ti.uid, ti.gid, ti.uname, ti.gname, ti.mode = MTIME, 0, 0, "", "", mode
            if kind == "d":
                ti.type = tarfile.DIRTYPE
                tf.addfile(ti)
            else:
                with open(os.path.join(root, rel), "rb") as f:
                    data = f.read()
                ti.size = len(data)
                tf.addfile(ti, io.BytesIO(data))
    return buf.getvalue()


def make_tar_empty() -> bytes:
    buf = io.BytesIO()
    with tarfile.open(fileobj=buf, mode="w", format=tarfile.PAX_FORMAT):
        pass
    return buf.getvalue()


def extract_tar(data: bytes, dest: str) -> None:
    os.makedirs(dest, exist_ok=True)
    with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as tf:
        for m in tf.getmembers():
            if m.name.startswith("/") or ".." in m.name.split("/") or not (m.isdir() or m.isreg()):
                raise PoolError("unsafe tar member %r" % m.name)
            p = os.path.join(dest, m.name)
            if m.isdir():
                os.makedirs(p, exist_ok=True)
            else:
                os.makedirs(os.path.dirname(p), exist_ok=True)
                with open(p, "wb") as f:
                    f.write(tf.extractfile(m).read())
                os.chmod(p, 0o755 if m.mode & 0o111 else 0o644)


def unittest_names(path: str) -> list:
    with open(path, "rb") as f:
        tree = ast.parse(f.read().decode("utf-8"))
    stem = os.path.splitext(os.path.basename(path))[0]
    names = []
    for node in tree.body:
        if isinstance(node, ast.ClassDef):
            for item in node.body:
                if isinstance(item, ast.FunctionDef) and item.name.startswith("test"):
                    names.append("%s.%s.%s" % (stem, node.name, item.name))
    return names


def cargo_names(path: str) -> list:
    with open(path, "rb") as f:
        text = f.read().decode("utf-8")
    if "#[ignore" in text:
        raise PoolError("%s: hidden Rust tests must not carry #[ignore]" % path)
    return re.findall(r"#\[test\]\s*(?:#\[[^\]]*\]\s*)*fn\s+([A-Za-z0-9_]+)", text)


def build_task(tid: str, out_root: str) -> str:
    src_dir = os.path.join(TASKS_DIR, tid)
    t = load_src(tid)
    fam = t["family"]
    if fam not in FAMILIES:
        raise PoolError("%s: family must be one of %s" % (tid, sorted(FAMILIES)))
    out = os.path.join(out_root, tid)
    if os.path.exists(out):
        raise PoolError("%s exists" % out)
    os.makedirs(os.path.join(out, "check"))
    work = tempfile.mkdtemp(prefix="pool-build-")
    try:
        ws = os.path.join(work, "ws")
        os.makedirs(ws)
        if os.path.isdir(os.path.join(src_dir, "workspace")):
            copy_tree(os.path.join(src_dir, "workspace"), ws)
        notice = os.path.join(src_dir, "NOTICE")
        derived = t["source"].get("kind") != "fresh"
        if derived and not os.path.isfile(notice):
            raise PoolError("%s: a derived task needs a NOTICE" % tid)
        if os.path.isfile(notice):
            shutil.copyfile(notice, os.path.join(ws, "NOTICE"))
            os.chmod(os.path.join(ws, "NOTICE"), 0o644)
            shutil.copyfile(notice, os.path.join(out, "NOTICE"))
        gen = os.path.join(src_dir, "workspace.gen")
        if os.path.isfile(gen):
            run_hook(["/bin/sh", gen, ws], ws, "%s: workspace.gen" % tid)
        ws_tar = make_tar(ws)
        sol = os.path.join(work, "sol")
        extract_tar(ws_tar, sol)
        if os.path.isdir(os.path.join(src_dir, "solution")):
            copy_tree(os.path.join(src_dir, "solution"), sol)
        solve = os.path.join(src_dir, "solve.sh")
        if os.path.isfile(solve):
            run_hook(["/bin/sh", solve], sol, "%s: solve.sh" % tid)
        for rel in t.get("solution_delete", []):
            os.unlink(os.path.join(sol, rel))
        sol_tar = make_tar(sol)
        # check/
        cd = os.path.join(out, "check")
        with open(os.path.join(cd, "run"), "w") as f:
            f.write(RUN_SH)
        os.chmod(os.path.join(cd, "run"), 0o755)
        shutil.copyfile(os.path.join(TOOLS_DIR, "poolcheck.py"), os.path.join(cd, "poolcheck.py"))
        os.chmod(os.path.join(cd, "poolcheck.py"), 0o644)
        if os.path.isdir(os.path.join(src_dir, "hidden")):
            copy_tree(os.path.join(src_dir, "hidden"), os.path.join(cd, "hidden"))
            for dirpath, _, filenames in os.walk(os.path.join(cd, "hidden")):
                for name in filenames:
                    os.chmod(os.path.join(dirpath, name), 0o644)
        steps = []
        for st in t["steps"]:
            st = dict(st)
            k = st.get("kind")
            if k == "py_unittest":
                if "names" not in st:
                    st["names"] = [n for f in st["tests"] for n in unittest_names(os.path.join(cd, f))]
            elif k == "cargo_test":
                if "names" not in st:
                    st["names"] = [n for _, f in sorted(st["tests"].items()) for n in cargo_names(os.path.join(cd, f))]
            elif k == "py_cases":
                cases = st["cases"]
                if isinstance(cases, str):
                    with open(os.path.join(cd, cases), "rb") as f:
                        cases = json.loads(f.read().decode("utf-8"))
                if not cases:
                    raise PoolError("%s: a py_cases step has no cases" % tid)
            elif k != "sh":
                raise PoolError("%s: unknown step kind %r" % (tid, k))
            if k != "py_cases" and not st.get("names"):
                raise PoolError("%s: a %s step has no test names" % (tid, k))
            steps.append(st)
        check = {"v": 1, "runner": "poolcheck/1", "scoring": t.get("scoring", "fraction"), "steps": steps}
        with open(os.path.join(cd, "check.json"), "wb") as f:
            f.write(dumps(check))
        prot = []
        for rel in sorted(t.get("protected", [])):
            p = os.path.join(ws, rel)
            if not os.path.isfile(p):
                raise PoolError("%s: protected path %s is not in the workspace" % (tid, rel))
            with open(p, "rb") as f:
                prot.append({"path": rel, "sha256": sha256(f.read())})
        with open(os.path.join(cd, "protected.json"), "wb") as f:
            f.write(dumps(prot))
        labels = list(t["labels"])
        for lb in labels:
            if not LABEL_RE.match(lb):
                raise PoolError("%s: bad label %r" % (tid, lb))
        limits = dict(LIMITS, max_turns=t.get("max_turns", FAMILIES[fam]))
        task = {
            "v": 3, "id": tid, "title": t["title"], "prompt": t["prompt"], "labels": labels,
            "workspace": {"file": "workspace.tar", "sha256": sha256(ws_tar), "bytes": len(ws_tar)},
            "check": {"argv": ["check/run"], "timeout_s": CHECK_TIMEOUT_S},
            "limits": limits, "source": {"kind": "maintainer"}, "created": CREATED,
            "family": fam, "template": tid, "difficulty": t["difficulty"],
        }
        with open(os.path.join(out, "task.json"), "wb") as f:
            f.write(dumps(task))
        with open(os.path.join(out, "workspace.tar"), "wb") as f:
            f.write(ws_tar)
        with open(os.path.join(out, "solution.tar"), "wb") as f:
            f.write(sol_tar)
    finally:
        shutil.rmtree(work, ignore_errors=True)
    return out


def build_all(out_root: str, only: list | None = None) -> list:
    ids = only or task_ids()
    os.makedirs(out_root, exist_ok=True)
    return [build_task(tid, out_root) for tid in ids]


# --------------------------------------------------------------------------------------------- checks


def default_toolchain() -> str | None:
    try:
        p = subprocess.run(["rustc", "--print", "sysroot"], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                           timeout=30)
    except (OSError, subprocess.TimeoutExpired):
        return None
    if p.returncode != 0:
        return None
    b = os.path.join(p.stdout.decode().strip(), "bin")
    return b if os.path.isfile(os.path.join(b, "cargo")) else None


def run_check(task_dir: str, ws_tar: bytes, toolchain: str | None, cargo_target: str | None,
              scratch: str) -> tuple:
    """Runs the task's check on a workspace (kernel/gate.md §3.7.1 protocol, minus the sandbox).
    Returns (score_permille, tamper, exit code, wall seconds, stderr tail, last stdout line)."""
    out = tempfile.mkdtemp(prefix="pool-out-", dir=scratch)
    try:
        ws = os.path.join(out, "ws")
        extract_tar(ws_tar, ws)
        shutil.copytree(os.path.join(task_dir, "check"), os.path.join(out, "check"))
        for d in ("tmp", "home"):
            os.makedirs(os.path.join(out, d))
        with open(os.path.join(task_dir, "task.json"), "rb") as f:
            task = json.loads(f.read().decode("utf-8"))
        path = "/usr/bin:/bin"
        if toolchain:
            path = toolchain + ":" + path
        env = {"PATH": path, "HOME": os.path.join(out, "home"), "TMPDIR": os.path.join(out, "tmp"), "LANG": "C",
               "INIT_TASK_ID": task["id"], "INIT_CHECK_DIR": os.path.join(out, "check"),
               }  # the runner always builds Rust in a fresh target dir of its own
        t0 = time.monotonic()
        p = subprocess.run([os.path.join(out, "check", "run")], cwd=ws, env=env, stdin=subprocess.DEVNULL,
                           stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=task["check"]["timeout_s"],
                           start_new_session=True)
        wall = time.monotonic() - t0
        lines = p.stdout.decode("utf-8", "replace").strip().splitlines()
        last = lines[-1] if lines else ""
        score, tamper = 0, False
        try:
            j = json.loads(last)
            tamper = j.get("tamper") is True
            if p.returncode == 0 and isinstance(j.get("score_permille"), int):
                score = j["score_permille"]
        except ValueError:
            score = 1000 if p.returncode == 0 and not last else 0
        return score, tamper, p.returncode, wall, p.stderr.decode("utf-8", "replace")[-3000:], last
    finally:
        shutil.rmtree(out, ignore_errors=True)


def partial_tar(tid: str, ws_tar: bytes, scratch: str) -> bytes | None:
    """The initial workspace with the source's partial/ overlay (a dev fixture that passes some of the
    hidden tests; used by validation and the runner parity test), or None."""
    src = os.path.join(TASKS_DIR, tid, "partial")
    if not os.path.isdir(src):
        return None
    d = tempfile.mkdtemp(prefix="pool-partial-", dir=scratch)
    try:
        extract_tar(ws_tar, os.path.join(d, "ws"))
        copy_tree(src, os.path.join(d, "ws"))
        return make_tar(os.path.join(d, "ws"))
    finally:
        shutil.rmtree(d, ignore_errors=True)


def validate_task_json(task: dict) -> list:
    errs = []
    keys = {"v", "id", "title", "prompt", "labels", "workspace", "check", "limits", "source", "created", "family",
            "template", "difficulty"}
    if set(task) - keys - {"accepted"}:
        errs.append("unknown keys %s" % sorted(set(task) - keys))
    if task.get("v") != 3 or not ID_RE.match(task.get("id", "")):
        errs.append("v must be 3 and id match the pattern")
    if not 1 <= len(task.get("prompt", "")) <= 32768 or len(task.get("title", "")) > 120:
        errs.append("prompt 1..32768, title <= 120")
    if not task.get("labels") or len(task["labels"]) > 16 or len(set(task["labels"])) != len(task["labels"]):
        errs.append("labels: 1..16 unique")
    lim = task.get("limits", {})
    if not (30 <= lim.get("wall_s", 0) <= 7200 and 1000 <= lim.get("max_cost_musd", 0) <= 50_000_000
            and 1 <= lim.get("max_turns", 0) <= 100 and lim.get("max_output_tokens", 0) >= 1000
            and lim.get("max_input_tokens", 0) >= 10_000):
        errs.append("limits out of range")
    if lim.get("max_cost_musd", 0) < 78_643:
        errs.append("max_cost_musd below one request's reservation (78,643 µUSD, m7-design §3.1.3)")
    if task.get("check") != {"argv": ["check/run"], "timeout_s": CHECK_TIMEOUT_S}:
        errs.append("check must be check/run, %d s" % CHECK_TIMEOUT_S)
    if task.get("family") not in FAMILIES or task.get("template") != task.get("id") or task.get("difficulty") not in range(1, 6):
        errs.append("family/template/difficulty")
    return errs


def validate_source(t: dict) -> list:
    errs = []
    s = t.get("source", {})
    kind = s.get("kind")
    if kind == "exercism":
        repo, slug = s.get("repo"), s.get("slug", "")
        if repo not in ("exercism/python", "exercism/rust") or s.get("commit") != SOURCES[repo][0]:
            errs.append("exercism source must be a pinned track commit")
        if not slug or not "n" <= slug[0] <= "z":
            errs.append("Exercism slug %r: pool slugs start with n-z (a-m belongs to the sealed E lanes, M7-P08)" % slug)
        if slug in POLYGLOT:
            errs.append("Exercism slug %r is in the polyglot benchmark slice" % slug)
        if t.get("family") != "e":
            errs.append("an Exercism task is family e")
    elif kind == "cruxeval":
        n = s.get("sample")
        if s.get("repo") != "facebookresearch/cruxeval" or s.get("commit") != SOURCES["facebookresearch/cruxeval"][0]:
            errs.append("cruxeval source must be the pinned commit")
        if not isinstance(n, int) or n in CRUX_SEALED:
            errs.append("CRUXEval sample_%s is used by the sealed C lane" % n)
        if t.get("family") != "c":
            errs.append("a CRUXEval task is family c")
    elif kind == "fresh":
        if t.get("family") != "t":
            errs.append("fresh tasks here are terminal tasks (family t)")
    else:
        errs.append("source.kind must be exercism, cruxeval or fresh")
    return errs


def same_tree(a: str, b: str) -> list:
    diffs = []
    fa = {r: (k, m) for r, k, m in tree_files(a)}
    fb = {r: (k, m) for r, k, m in tree_files(b)}
    for rel in sorted(set(fa) | set(fb)):
        if fa.get(rel) != fb.get(rel):
            diffs.append(rel)
        elif fa[rel][0] == "f":
            with open(os.path.join(a, rel), "rb") as x, open(os.path.join(b, rel), "rb") as y:
                if x.read() != y.read():
                    diffs.append(rel)
    return diffs


def validate(ids: list, repeat: int, toolchain: str | None, cargo_target: str | None, scratch: str,
             built: str | None = None) -> list:
    """One report per task: {id, ok, errors[], initial[], solution[], max_wall_ms, tamper_ok}."""
    b1 = built or os.path.join(scratch, "b1")
    if built is None:
        build_all(b1, ids)
    b2 = os.path.join(scratch, "b2")
    build_all(b2, ids)
    seen_slugs = {}
    reports = []
    for tid in ids:
        t = load_src(tid)
        errs = []
        d = same_tree(os.path.join(b1, tid), os.path.join(b2, tid))
        if d:
            errs.append("build is not deterministic: %s" % ", ".join(d[:5]))
        td = os.path.join(b1, tid)
        with open(os.path.join(td, "task.json"), "rb") as f:
            task = json.loads(f.read().decode("utf-8"))
        errs += validate_task_json(task)
        errs += validate_source(t)
        s = t.get("source", {})
        if s.get("kind") in ("exercism", "cruxeval"):
            key = (s.get("repo"), s.get("slug") or s.get("sample"))
            if key in seen_slugs:
                errs.append("same source as %s" % seen_slugs[key])
            seen_slugs[key] = tid
        with open(os.path.join(td, "workspace.tar"), "rb") as f:
            ws_tar = f.read()
        with open(os.path.join(td, "solution.tar"), "rb") as f:
            sol_tar = f.read()
        if sha256(ws_tar) != task["workspace"]["sha256"] or len(ws_tar) != task["workspace"]["bytes"]:
            errs.append("workspace.tar does not match task.json")
        init_scores, sol_scores, walls = [], [], []
        for _ in range(repeat):
            sc, tamper, code, wall, err, _ = run_check(td, ws_tar, toolchain, cargo_target, scratch)
            init_scores.append(sc)
            walls.append(wall)
            if tamper:
                errs.append("the initial workspace reads as tampered")
        for _ in range(repeat):
            sc, tamper, code, wall, err, _ = run_check(td, sol_tar, toolchain, cargo_target, scratch)
            sol_scores.append(sc)
            walls.append(wall)
            if sc != 1000:
                errs.append("reference solution scored %d (exit %d): %s" % (sc, code, err[-800:]))
        if any(s > 500 for s in init_scores):
            errs.append("no-op scores %s: the initial workspace must score <= 500 (s1 validator rule)" % init_scores)
        sc, tamper, code, wall, err, _ = run_check(td, make_tar_empty(), toolchain, cargo_target, scratch)
        if sc >= 1000:
            errs.append("an empty workspace scores 1000")
        part = partial_tar(tid, ws_tar, scratch)
        partial_scores = []
        if part is not None:
            for _ in range(min(repeat, 2)):
                sc, tamper, code, wall, err, _ = run_check(td, part, toolchain, cargo_target, scratch)
                partial_scores.append(sc)
                walls.append(wall)
            if not all(0 < x < 1000 for x in partial_scores) or len(set(partial_scores)) > 1:
                errs.append("partial fixture scores %s (want one value strictly between 0 and 1000)" % partial_scores)
        if len(set(init_scores)) > 1 or len(set(sol_scores)) > 1:
            errs.append("flaky_check: initial %s, solution %s" % (init_scores, sol_scores))
        if max(walls) > CHECK_WALL_MAX_S:
            errs.append("a check took %.1f s (> %.0f s)" % (max(walls), CHECK_WALL_MAX_S))
        tamper_ok = None
        with open(os.path.join(td, "check", "protected.json"), "rb") as f:
            prot = json.loads(f.read().decode("utf-8"))
        if prot:
            tws = os.path.join(scratch, "tamper-ws")
            extract_tar(sol_tar, tws)
            with open(os.path.join(tws, prot[0]["path"]), "ab") as f:
                f.write(b"\n# edited\n")
            sc, tamper, code, wall, err, _ = run_check(td, make_tar(tws), toolchain, cargo_target, scratch)
            shutil.rmtree(tws)
            tamper_ok = tamper and sc == 0 and code == 3
            if not tamper_ok:
                errs.append("editing protected %s did not score tamper (score %d, exit %d)" % (prot[0]["path"], sc, code))
        reports.append({"id": tid, "ok": not errs, "errors": errs, "initial": init_scores, "solution": sol_scores,
                        "partial": partial_scores,
                        "max_wall_ms": int(max(walls) * 1000), "tamper_ok": tamper_ok,
                        "family": t["family"], "difficulty": t["difficulty"]})
    return reports


# ------------------------------------------------------------------------------------------ py cases


def gen_cases(tid: str) -> int:
    """Writes tasks/<id>/hidden/cases.json from tasks/<id>/cases.in.json ({"import", "cases": [[name, expr],
    ...]}) by evaluating every expression against the reference solution through poolcheck's own child
    (PYTHONHASHSEED=0), so the expected reprs are exactly what the check will compare."""
    src_dir = os.path.join(TASKS_DIR, tid)
    with open(os.path.join(src_dir, "cases.in.json"), "rb") as f:
        spec = json.loads(f.read().decode("utf-8"))
    target = os.path.join(src_dir, "hidden", "cases.json")
    if not os.path.exists(target):  # the first build needs a cases file to read
        os.makedirs(os.path.dirname(target), exist_ok=True)
        with open(target, "wb") as f:
            f.write(dumps([{"name": "placeholder", "expr": "0", "expect": "0"}]))
    scratch = tempfile.mkdtemp(prefix="pool-cases-")
    try:
        built = build_task(tid, scratch)
        with open(os.path.join(built, "solution.tar"), "rb") as f:
            sol = f.read()
        ws = os.path.join(scratch, "ws")
        extract_tar(sol, ws)
        names = [n for n, _ in spec["cases"]]
        if len(set(names)) != len(names):
            raise PoolError("%s: duplicate case names" % tid)
        job = json.dumps({"ws": ws, "import": spec["import"], "exprs": [e for _, e in spec["cases"]],
                          "timeouts": [5] * len(names)}).encode()
        env = {"PATH": "/usr/bin:/bin", "HOME": scratch, "TMPDIR": scratch, "LC_ALL": "C", "PYTHONHASHSEED": "0",
               "PYTHONDONTWRITEBYTECODE": "1", "PYTHONNOUSERSITE": "1", "PYTHONIOENCODING": "utf-8"}
        p = subprocess.run([sys.executable, "-S", "-s", "-B", os.path.join(TOOLS_DIR, "poolcheck.py"),
                            "--child-py-cases"], input=job, stdout=subprocess.PIPE, cwd=ws, env=env, timeout=120)
        res = [json.loads(x) for x in p.stdout.decode("utf-8").splitlines()]
        if len(res) != len(names):
            raise PoolError("%s: reference evaluation returned %d of %d results" % (tid, len(res), len(names)))
        cases = []
        for (name, expr), r in zip(spec["cases"], res):
            if r["ok"]:
                ast.literal_eval(r["repr"])  # must be a literal the runner can read back
                cases.append({"name": name, "expr": expr, "expect": r["repr"]})
            else:
                cases.append({"name": name, "expr": expr, "raises": r["exc"][0]})
        os.makedirs(os.path.join(src_dir, "hidden"), exist_ok=True)
        with open(os.path.join(src_dir, "hidden", "cases.json"), "wb") as f:
            f.write(dumps(cases))
        return len(cases)
    finally:
        make_writable(scratch)
        shutil.rmtree(scratch, ignore_errors=True)


# ------------------------------------------------------------------------------------------- manifest


def manifest() -> dict:
    tasks = []
    for tid in task_ids():
        t = load_src(tid)
        tasks.append({"id": tid, "title": t["title"], "family": t["family"], "labels": t["labels"],
                      "difficulty": t["difficulty"], "source": t["source"]})
    counts = {}
    for x in tasks:
        counts[x["family"]] = counts.get(x["family"], 0) + 1
    return {
        "v": 1, "pool": POOL_ID, "doc": "docs/m7-design.md §3.3; build/specs/PROOF-LEAN.md D-L4",
        "composition": counts, "tasks": tasks,
        "licences": {"exercism": "LICENSES/Exercism-MIT.txt", "cruxeval": "LICENSES/CRUXEval-MIT.txt",
                     "fresh": "written fresh for INIT (INIT licence)"},
        "pins": {repo: c for repo, (c, _) in sorted(SOURCES.items())},
        "disjoint": {"exercism_slugs": "n-z only (a-m: sealed E lanes)", "polyglot": sorted(POLYGLOT),
                     "crux_sealed_samples": sorted(CRUX_SEALED)},
    }


# --------------------------------------------------------------------------------------------- import


def make_writable(root: str) -> None:
    if not os.path.lexists(root):
        return
    for dirpath, dirnames, filenames in os.walk(root):
        os.chmod(dirpath, 0o755)
        for n in filenames:
            p = os.path.join(dirpath, n)
            if not os.path.islink(p):
                os.chmod(p, 0o644)


def set_modes(root: str, dmode: int, fmode: int, xmode: int) -> None:
    for dirpath, dirnames, filenames in os.walk(root, topdown=False):
        for n in filenames:
            p = os.path.join(dirpath, n)
            os.chmod(p, xmode if os.stat(p).st_mode & 0o111 else fmode)
        os.chmod(dirpath, dmode)


def frozen_equal(src: str, dst: str) -> bool:
    """dst (an earlier import) holds the same task as src, ignoring task.json's `accepted`."""
    try:
        with open(os.path.join(dst, "task.json"), "rb") as f:
            old = json.loads(f.read().decode("utf-8"))
        with open(os.path.join(src, "task.json"), "rb") as f:
            new = json.loads(f.read().decode("utf-8"))
    except (OSError, ValueError):
        return False
    old.pop("accepted", None)
    if old != new:
        return False
    a = [(r, k) for r, k, _ in tree_files(src) if r != "task.json"]
    b = [(r, k) for r, k, _ in tree_files(dst) if r != "task.json"]
    if a != b:
        return False
    return not [r for r in same_tree(src, dst) if r != "task.json"]


def guard_home(home: str) -> str:
    real = os.path.realpath(home)
    for live in ("~/.init", "~/.init-root"):
        if real == os.path.realpath(os.path.expanduser(live)):
            raise PoolError("refusing to import into the owner's live home %s (use the kernel's own import)" % live)
    if not os.path.isdir(real):
        raise PoolError("%s is not a directory" % home)
    return real


def do_import(home: str, toolchain: str | None, cargo_target: str | None, replace: bool, repeat: int) -> dict:
    home = guard_home(home)
    scratch = tempfile.mkdtemp(prefix="pool-import-")
    try:
        ids = task_ids()
        b1 = os.path.join(scratch, "b1")
        build_all(b1, ids)
        reports = validate(ids, repeat, toolchain, cargo_target, scratch, built=b1)
        bad = [r for r in reports if not r["ok"]]
        if bad:
            raise PoolError("validation failed: %s" % json.dumps(bad, sort_keys=True)[:4000])
        vault = os.path.join(home, "vault", "gate", "pool")
        mirror = os.path.join(home, "kernel", "pool")
        os.makedirs(vault, exist_ok=True)
        os.chmod(os.path.join(home, "vault"), 0o700)
        now = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%S.%f")[:-3] + "Z"
        added, same = [], []
        for tid in ids:
            src = os.path.join(b1, tid)
            dst = os.path.join(vault, tid)
            if os.path.lexists(dst):
                if frozen_equal(src, dst):
                    same.append(tid)
                    continue
                if not replace:
                    raise PoolError("%s is already frozen with different content (use --replace)" % tid)
                make_writable(dst)
                shutil.rmtree(dst)
            tmp = dst + ".tmp"
            if os.path.lexists(tmp):
                make_writable(tmp)
                shutil.rmtree(tmp)
            shutil.copytree(src, tmp)
            with open(os.path.join(tmp, "task.json"), "rb") as f:
                task = json.loads(f.read().decode("utf-8"))
            task["accepted"] = {"by": "maintainer", "at": now}
            with open(os.path.join(tmp, "task.json"), "wb") as f:
                f.write(dumps(task))
            set_modes(tmp, 0o700, 0o600, 0o700)
            os.rename(tmp, dst)
            added.append(tid)
        # the read-only mirror the agent may read: no solution.tar
        if os.path.lexists(mirror):
            make_writable(mirror)
            shutil.rmtree(mirror)
        os.makedirs(mirror)
        for tid in ids:
            m = os.path.join(mirror, tid)
            os.makedirs(m)
            for name in ("task.json", "workspace.tar", "NOTICE"):
                p = os.path.join(vault, tid, name)
                if os.path.exists(p):
                    shutil.copyfile(p, os.path.join(m, name))
            shutil.copytree(os.path.join(vault, tid, "check"), os.path.join(m, "check"))
        shutil.copytree(os.path.join(POOL_DIR, "LICENSES"), os.path.join(mirror, "LICENSES"))
        with open(os.path.join(mirror, "pool.json"), "wb") as f:
            f.write(dumps(manifest()))
        set_modes(mirror, 0o555, 0o444, 0o555)
        rec = {"v": 1, "ts": now, "kind": "pool_import", "pool": POOL_ID, "added": added, "unchanged": same,
               "manifest_sha256": sha256(dumps(manifest()))}
        with open(os.path.join(home, "vault", "gate", "pool-import.jsonl"), "ab") as f:
            f.write((json.dumps(rec, sort_keys=True) + "\n").encode("ascii"))
        return {"added": added, "unchanged": same, "vault": vault, "mirror": mirror, "reports": reports}
    finally:
        make_writable(scratch)
        shutil.rmtree(scratch, ignore_errors=True)


# ------------------------------------------------------------------------------------------------ CLI


def main(argv: list) -> int:
    ap = argparse.ArgumentParser(prog="pool.py", description=__doc__.split("\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    for name in ("build", "validate", "import", "manifest", "cases"):
        p = sub.add_parser(name)
        p.add_argument("--only", default="")
        p.add_argument("--toolchain", default=None)
        p.add_argument("--cargo-target-dir", default=None)
        p.add_argument("--repeat", type=int, default=3)
        p.add_argument("--json", action="store_true")
        if name == "build":
            p.add_argument("--out", required=True)
        if name == "import":
            p.add_argument("--home", required=True)
            p.add_argument("--replace", action="store_true")
        if name == "manifest":
            p.add_argument("--check", action="store_true")
    a = ap.parse_args(argv[1:])
    only = [x for x in a.only.split(",") if x] or None
    toolchain = a.toolchain or default_toolchain()
    try:
        if a.cmd == "build":
            for p in build_all(a.out, only):
                print(p)
            return 0
        if a.cmd == "cases":
            for tid in only or [t for t in task_ids() if os.path.isfile(os.path.join(TASKS_DIR, t, "cases.in.json"))]:
                print("%s: %d cases" % (tid, gen_cases(tid)))
            return 0
        if a.cmd == "manifest":
            want = dumps(manifest())
            path = os.path.join(POOL_DIR, "pool.json")
            if a.check:
                have = open(path, "rb").read() if os.path.exists(path) else b""
                if have != want:
                    say("pool.json is stale: run pool.py manifest")
                    return 1
                return 0
            with open(path, "wb") as f:
                f.write(want)
            return 0
        if a.cmd == "validate":
            scratch = tempfile.mkdtemp(prefix="pool-validate-")
            try:
                reports = validate(only or task_ids(), a.repeat, toolchain, a.cargo_target_dir, scratch)
            finally:
                make_writable(scratch)
                shutil.rmtree(scratch, ignore_errors=True)
            if a.json:
                sys.stdout.write(dumps(reports).decode("ascii"))
            else:
                for r in reports:
                    print("%-34s %s  fam=%s d=%d initial=%s solution=%s partial=%s max_wall=%dms tamper=%s" % (
                        r["id"], "ok  " if r["ok"] else "FAIL", r["family"], r["difficulty"], r["initial"],
                        r["solution"], r["partial"], r["max_wall_ms"], r["tamper_ok"]))
                    for e in r["errors"]:
                        print("    - " + e)
                print("%d/%d tasks valid" % (sum(r["ok"] for r in reports), len(reports)))
            return 0 if all(r["ok"] for r in reports) else 1
        if a.cmd == "import":
            res = do_import(a.home, toolchain, a.cargo_target_dir, a.replace, a.repeat)
            print("imported %d, unchanged %d into %s; mirror %s" % (len(res["added"]), len(res["unchanged"]),
                                                                    res["vault"], res["mirror"]))
            return 0
    except PoolError as e:
        say("pool.py: %s" % e)
        return 1
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv))
