#!/usr/bin/env python3
"""INIT sealed pack s1: the shared check runner (docs/m7-design.md §3.1.3). Python >= 3.9, stdlib only.

One audited code path for every S task. `check/run` (runner/check) execs:

    python3 -I -S -B <check_dir>/s1check.py <check_dir>

with cwd = the workspace (OUT/ws), after the kernel/Root has prepared OUT per kernel/gate.md §3.7.1.
It reads <check_dir>/check.json (what to run) and <check_dir>/protected.json (what must not change).

Output: the LAST stdout line is the score line, and the exit code says how to read it:
  exit 0  {"score_permille":N}                 the score
  exit 3  {"score_permille":0,"tamper":true}   a protected file was changed, deleted or replaced by a link,
                                               or a workspace link points into the check dir. The non-zero
                                               exit makes every pre-v3 parser (which reads "exit 0 and an
                                               unknown last line" as 1000) score it 0; a v3 parser reads
                                               the tamper key from the last line whatever the exit code.
  exit 2  {"score_permille":0}                 check.json / protected.json malformed (a template bug)
  exit 4  {"score_permille":0}                 a toolchain the check needs is missing (draw skips the template)
With "on_tamper":"score_1000" (the tamper canary only) a tamper prints {"score_permille":1000} with exit 0.

The runner never writes into the check dir (the Root digests it before and after; §3.1.3 item 3) and never
lets workspace code see expected values: Python values cross back as repr() and are compared here.

Internal modes (the runner's own children and the generators' reference evaluation):
  --child-py-case        a py_cases step: job JSON on stdin, one result JSON line per case, in order
  --child-py-unittest    one py_unittest step: job JSON on stdin, one JSON line per finished test
  --child-py-batch       generators only: evaluate many expressions against trusted reference code
"""
from __future__ import annotations

import ast
import hashlib
import json
import math
import os
import re
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import threading
import time

RUNNER_ID = "s1check/1"
EXIT_OK, EXIT_SPEC, EXIT_TAMPER, EXIT_TOOL = 0, 2, 3, 4
CAP_OUT = 4 << 20          # bytes kept from any child's stdout
CAP_FILE = 64 << 20        # largest workspace file the runner reads
CAP_WALK = 200_000         # workspace entries scanned for links into the check dir
DEFAULT_BUDGET_S = 50      # all steps together; the check timeout for S tasks is 60 s
SCORINGS = ("fraction", "all", "any")
COMPARES = ("exact", "strip", "lines", "contains")
ON_TAMPER = ("zero", "score_1000")
HEX64 = re.compile(r"^[0-9a-f]{64}$")
NAME = re.compile(r"^[A-Za-z0-9_.:/-]{1,200}$")
PY_IMPORT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*(\.[A-Za-z_][A-Za-z0-9_]*)*$")
CARGO_LINE = re.compile(r"^test (\S+) \.\.\. (ok|FAILED|ignored)\s*$")
SH_LINE = re.compile(r"^(PASS|FAIL) (\S+)\s*$")


class SpecError(Exception):
    """check.json or protected.json is malformed (exit 2)."""


class ToolMissing(Exception):
    """A toolchain the check needs is not on PATH (exit 4)."""


def log(msg: str) -> None:
    sys.stderr.write("s1check: " + msg + "\n")


def score_line(score: int, tamper: bool = False) -> str:
    if tamper:
        return '{"score_permille":0,"tamper":true}'
    return '{"score_permille":%d}' % score


def finish(score: int, tamper: bool = False, code: int = EXIT_OK) -> None:
    sys.stdout.write(score_line(score, tamper) + "\n")
    sys.stdout.flush()
    sys.exit(code)


# ---------------------------------------------------------------------------------------------- paths


def rel_ok(p: object) -> bool:
    """A safe relative path: '/'-separated, no empty, '.' or '..' component, no NUL or backslash."""
    if not isinstance(p, str) or not p or len(p) > 512 or "\0" in p or "\\" in p or p.startswith("/"):
        return False
    return all(part not in ("", ".", "..") for part in p.split("/"))


def read_ws(root: str, rel: str, cap: int = CAP_FILE) -> tuple[str, bytes | None]:
    """Reads root/rel without following a symlink at any component.
    Returns (state, data); state is ok | missing | link | notfile | big."""
    cur = root
    parts = rel.split("/")
    st = None
    for i, part in enumerate(parts):
        cur = os.path.join(cur, part)
        try:
            st = os.lstat(cur)
        except (FileNotFoundError, NotADirectoryError):
            return "missing", None
        if stat.S_ISLNK(st.st_mode):
            return "link", None
        if i < len(parts) - 1 and not stat.S_ISDIR(st.st_mode):
            return "missing", None
    if st is None or not stat.S_ISREG(st.st_mode):
        return "notfile", None
    if st.st_size > cap:
        return "big", None
    try:
        fd = os.open(cur, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    except OSError:
        return "link", None
    with os.fdopen(fd, "rb") as f:
        return "ok", f.read(cap + 1)


def check_file(cd: str, rel: str) -> str:
    """A path inside the check dir named by check.json; must exist."""
    if not rel_ok(rel):
        raise SpecError("bad check-dir path: %r" % (rel,))
    p = os.path.join(cd, rel)
    if not os.path.isfile(p):
        raise SpecError("missing check-dir file: %s" % rel)
    return p


def read_check(cd: str, rel: str) -> bytes:
    with open(check_file(cd, rel), "rb") as f:
        return f.read()


def sha256_bytes(b: bytes) -> str:
    return hashlib.sha256(b).hexdigest()


def load_json_file(path: str) -> object:
    try:
        with open(path, "rb") as f:
            return json.loads(f.read().decode("utf-8"))
    except (OSError, ValueError) as e:
        raise SpecError("%s: %s" % (os.path.basename(path), e))


# ------------------------------------------------------------------------------------------ the spec

STEP_KEYS = {
    "files": ({"kind", "items"}, {"name", "weight"}),
    "run": ({"kind", "cases"}, {"name", "weight", "inputs"}),
    "py_cases": ({"kind", "import", "cases"}, {"name", "weight", "timeout_s"}),
    "py_unittest": ({"kind", "tests", "names"}, {"name", "weight", "timeout_s"}),
    "cargo_test": ({"kind", "manifest", "tests", "names"}, {"name", "weight", "timeout_s"}),
    "sh": ({"kind", "script", "names"}, {"name", "weight", "timeout_s"}),
}
TOP_KEYS = ({"v", "runner", "scoring", "steps"}, {"on_tamper", "budget_s"})


def keys_ok(obj: object, required: set, optional: set, what: str) -> dict:
    if not isinstance(obj, dict):
        raise SpecError("%s: not an object" % what)
    missing = required - set(obj)
    extra = set(obj) - required - optional
    if missing or extra:
        raise SpecError("%s: missing %s, unknown %s" % (what, sorted(missing), sorted(extra)))
    return obj


def int_in(v: object, lo: int, hi: int, what: str) -> int:
    if not isinstance(v, int) or isinstance(v, bool) or not lo <= v <= hi:
        raise SpecError("%s: integer %d..%d expected" % (what, lo, hi))
    return v


def names_ok(v: object, what: str) -> list:
    if not isinstance(v, list) or not v or not all(isinstance(n, str) and NAME.match(n) for n in v):
        raise SpecError("%s: non-empty list of names expected" % what)
    if len(set(v)) != len(v):
        raise SpecError("%s: duplicate names" % what)
    return v


def load_spec(cd: str) -> dict:
    spec = keys_ok(load_json_file(os.path.join(cd, "check.json")), TOP_KEYS[0], TOP_KEYS[1], "check.json")
    if spec["v"] != 1 or spec["runner"] != RUNNER_ID:
        raise SpecError("check.json: v must be 1 and runner %s" % RUNNER_ID)
    if spec["scoring"] not in SCORINGS:
        raise SpecError("check.json: scoring must be one of %s" % (SCORINGS,))
    spec.setdefault("on_tamper", "zero")
    if spec["on_tamper"] not in ON_TAMPER:
        raise SpecError("check.json: on_tamper must be one of %s" % (ON_TAMPER,))
    spec["budget_s"] = int_in(spec.get("budget_s", DEFAULT_BUDGET_S), 1, 1700, "check.json budget_s")
    if not isinstance(spec["steps"], list):
        raise SpecError("check.json: steps must be a list")
    for i, step in enumerate(spec["steps"]):
        kind = step.get("kind") if isinstance(step, dict) else None
        if kind not in STEP_KEYS:
            raise SpecError("step %d: unknown kind %r" % (i, kind))
        keys_ok(step, STEP_KEYS[kind][0], STEP_KEYS[kind][1], "step %d (%s)" % (i, kind))
        int_in(step.get("weight", 1), 1, 1000, "step %d weight" % i)
        if "timeout_s" in step:
            int_in(step["timeout_s"], 1, 1700, "step %d timeout_s" % i)
    return spec


def load_protected(cd: str) -> list:
    data = load_json_file(os.path.join(cd, "protected.json"))
    if not isinstance(data, list):
        raise SpecError("protected.json: list expected")
    seen = set()
    for e in data:
        keys_ok(e, {"path", "sha256"}, set(), "protected.json entry")
        if not rel_ok(e["path"]) or not isinstance(e["sha256"], str) or not HEX64.match(e["sha256"]):
            raise SpecError("protected.json: bad entry %r" % (e,))
        if e["path"] in seen:
            raise SpecError("protected.json: duplicate %s" % e["path"])
        seen.add(e["path"])
    return data


# ---------------------------------------------------------------------------------------- tampering


def links_into(ws: str, cd: str) -> list:
    """Workspace symlinks that resolve into the check dir (only a check-time reader would plant one)."""
    cd_real = os.path.realpath(cd)
    out, n = [], 0
    for dirpath, dirnames, filenames in os.walk(ws):
        for name in dirnames + filenames:
            n += 1
            if n > CAP_WALK:
                return out
            p = os.path.join(dirpath, name)
            if os.path.islink(p):
                t = os.path.realpath(p)
                if t == cd_real or t.startswith(cd_real + os.sep):
                    out.append("link into the check dir: " + os.path.relpath(p, ws))
    return out


def tamper_reasons(ws: str, cd: str, protected: list) -> list:
    reasons = []
    for e in protected:
        state, data = read_ws(ws, e["path"])
        if state != "ok":
            reasons.append("%s: %s" % (state, e["path"]))
        elif sha256_bytes(data) != e["sha256"]:
            reasons.append("changed: %s" % e["path"])
    return reasons + links_into(ws, cd)


# ------------------------------------------------------------------------------------ child processes


def child_env(drop_check_dir: bool = True) -> dict:
    """The check's environment (already env_clear'ed by the Root/kernel) minus Python and Cargo knobs,
    plus a fixed locale and hash seed so that results are deterministic."""
    env = {}
    for k, v in os.environ.items():
        if k.startswith(("PYTHON", "RUSTC", "RUSTFLAGS", "RUSTDOC", "CARGO_BUILD", "CARGO_ENCODED")):
            continue
        if drop_check_dir and k == "INIT_CHECK_DIR":
            continue
        env[k] = v
    env.update({"LC_ALL": "C", "PYTHONHASHSEED": "0", "PYTHONDONTWRITEBYTECODE": "1",
                "PYTHONNOUSERSITE": "1", "PYTHONIOENCODING": "utf-8"})
    return env


def _drain(fd: int, buf: bytearray) -> None:
    while True:
        try:
            chunk = os.read(fd, 65536)
        except OSError:
            return
        if not chunk:
            return
        room = CAP_OUT - len(buf)
        if room > 0:
            buf.extend(chunk[:room])


def _feed(f, data: bytes) -> None:
    try:
        f.write(data)
    except OSError:
        pass
    finally:
        try:
            f.close()
        except OSError:
            pass


def spawn(argv: list, cwd: str, env: dict, stdin: bytes | None, timeout: float) -> tuple:
    """Runs argv in its own process group. Returns (exit code or None on timeout, stdout bytes).
    On timeout the group (this runner's own child, by its pid) is killed."""
    if timeout <= 0:
        return None, b""
    try:
        p = subprocess.Popen(argv, cwd=cwd, env=env, stdin=subprocess.PIPE if stdin is not None else subprocess.DEVNULL,
                             stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, start_new_session=True)
    except OSError as e:
        log("spawn %s: %s" % (argv[0], e))
        return -1, b""
    buf = bytearray()
    reader = threading.Thread(target=_drain, args=(p.stdout.fileno(), buf), daemon=True)
    reader.start()
    if stdin is not None:
        threading.Thread(target=_feed, args=(p.stdin, stdin), daemon=True).start()
    exited = _wait_unreaped(p, timeout)
    if p.returncode is None:
        # The leader is alive or an unreaped zombie, so its pid (our group id) cannot have been reused:
        # killing the group reaches only this runner's own child and what it left behind.
        try:
            os.killpg(p.pid, signal.SIGKILL)
        except OSError:
            pass
    code = p.wait()
    reader.join(2.0)  # a process that left the group may hold the pipe; never wait on it
    try:
        p.stdout.close()
    except OSError:
        pass
    return (code if exited else None), bytes(buf)


def _wait_unreaped(p: subprocess.Popen, timeout: float) -> bool:
    """Waits until the child exits or the timeout passes, without reaping it where the OS allows
    (os.waitid with WNOWAIT); returns whether it exited in time."""
    if not hasattr(os, "waitid"):
        try:
            p.wait(timeout=timeout)  # reaps: the group is then left to the check instance's own teardown
            return True
        except subprocess.TimeoutExpired:
            return False
    end = time.monotonic() + timeout
    nap = 0.002
    while True:
        try:
            if os.waitid(os.P_PID, p.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT) is not None:
                return True
        except ChildProcessError:
            return True
        if time.monotonic() >= end:
            return False
        time.sleep(nap)
        nap = min(nap * 2, 0.02)


def remaining(deadline: float, cap: float) -> float:
    return min(cap, deadline - time.monotonic())


def py_child_argv() -> list:
    # no -I/-E: the env is built by child_env (PYTHONHASHSEED must apply); -S: no site-packages;
    # -s: no user site; -B: no bytecode written anywhere
    return [sys.executable, "-S", "-s", "-B", os.path.abspath(__file__)]


# ---------------------------------------------------------------------------------------- comparing


def same(a: object, b: object, approx: float | None = None) -> bool:
    """Structural equality that also requires equal types (True is not 1, 1 is not 1.0)."""
    if type(a) is not type(b):
        return False
    if isinstance(a, float):
        if math.isnan(a) and math.isnan(b):
            return True
        if approx is not None:
            return math.isclose(a, b, rel_tol=approx, abs_tol=approx)
        return a == b
    if isinstance(a, (list, tuple)):
        return len(a) == len(b) and all(same(x, y, approx) for x, y in zip(a, b))
    if isinstance(a, dict):
        return a.keys() == b.keys() and all(same(a[k], b[k], approx) for k in a)
    return a == b


def text_match(got: bytes, want: bytes, mode: str) -> bool:
    if mode == "exact":
        return got == want
    g, w = got.decode("utf-8", "replace"), want.decode("utf-8", "replace")
    if mode == "strip":
        return g.strip() == w.strip()
    if mode == "contains":
        return w.strip() != "" and w.strip() in g

    def norm(s: str) -> list:
        lines = [ln.rstrip() for ln in s.replace("\r\n", "\n").split("\n")]
        while lines and lines[-1] == "":
            lines.pop()
        return lines
    return norm(g) == norm(w)


def want_bytes(cd: str, item: dict, what: str) -> bytes:
    """An expected value: exactly one of `expect` (a check-dir file) or `text` (inline)."""
    has_file, has_text = "expect" in item, "text" in item
    if has_file == has_text:
        raise SpecError("%s: exactly one of expect/text" % what)
    if has_file:
        return read_check(cd, item["expect"])
    if not isinstance(item["text"], str):
        raise SpecError("%s: text must be a string" % what)
    return item["text"].encode("utf-8")


# -------------------------------------------------------------------------------------------- steps


def step_files(step: dict, ws: str, cd: str, scratch: str, deadline: float) -> list:
    if not isinstance(step["items"], list) or not step["items"]:
        raise SpecError("files: items must be a non-empty list")
    out = []
    for i, item in enumerate(step["items"]):
        keys_ok(item, {"path"}, {"name", "expect", "text", "compare"}, "files item %d" % i)
        if not rel_ok(item["path"]):
            raise SpecError("files item %d: bad path" % i)
        mode = item.get("compare", "exact")
        if mode not in COMPARES:
            raise SpecError("files item %d: compare" % i)
        want = want_bytes(cd, item, "files item %d" % i)
        state, got = read_ws(ws, item["path"])
        out.append((item.get("name", item["path"]), state == "ok" and text_match(got, want, mode)))
    return out


def step_run(step: dict, ws: str, cd: str, scratch: str, deadline: float) -> list:
    inputs = step.get("inputs", {})
    if not isinstance(inputs, dict) or not all(rel_ok(k) and isinstance(v, str) for k, v in inputs.items()):
        raise SpecError("run: inputs must map names to check-dir files")
    if not isinstance(step["cases"], list) or not step["cases"]:
        raise SpecError("run: cases must be a non-empty list")
    out = []
    for i, case in enumerate(step["cases"]):
        keys_ok(case, {"name", "argv"}, {"stdin", "expect", "text", "compare", "expect_exit", "timeout_s"}, "run case %d" % i)
        argv = case["argv"]
        if not isinstance(argv, list) or not argv or not all(isinstance(a, str) for a in argv):
            raise SpecError("run case %d: argv" % i)
        mode = case.get("compare", "lines")
        if mode not in COMPARES:
            raise SpecError("run case %d: compare" % i)
        want = want_bytes(cd, case, "run case %d" % i) if ("expect" in case or "text" in case) else None
        exp_exit = case.get("expect_exit", 0)
        if exp_exit is not None:
            int_in(exp_exit, 0, 255, "run case %d expect_exit" % i)
        timeout = int_in(case.get("timeout_s", 10), 1, 600, "run case %d timeout_s" % i)
        stdin = read_check(cd, case["stdin"]) if "stdin" in case else b""
        indir = tempfile.mkdtemp(prefix="in-", dir=scratch)
        for name, src in sorted(inputs.items()):
            dst = os.path.join(indir, name)
            os.makedirs(os.path.dirname(dst), exist_ok=True)
            shutil.copyfile(check_file(cd, src), dst)
        argv = [a.replace("{in}", indir) for a in argv]
        code, got = spawn(argv, ws, child_env(), stdin, remaining(deadline, timeout))
        ok = code is not None and (exp_exit is None or code == exp_exit)
        if ok and want is not None:
            ok = text_match(got, want, mode)
        out.append((case["name"], ok))
        shutil.rmtree(indir, ignore_errors=True)
    return out


def py_cases_list(step: dict, cd: str) -> list:
    cases = step["cases"]
    if isinstance(cases, str):
        cases = load_json_file(check_file(cd, cases))
    if not isinstance(cases, list) or not cases:
        raise SpecError("py_cases: cases must be a non-empty list (inline or a check-dir JSON file)")
    names = set()
    for i, c in enumerate(cases):
        keys_ok(c, {"name", "expr"}, {"expect", "raises", "approx", "timeout_s"}, "py case %d" % i)
        if ("expect" in c) == ("raises" in c):
            raise SpecError("py case %d: exactly one of expect/raises" % i)
        if not isinstance(c["name"], str) or not NAME.match(c["name"]) or c["name"] in names:
            raise SpecError("py case %d: bad or duplicate name" % i)
        names.add(c["name"])
        if not isinstance(c["expr"], str) or not c["expr"]:
            raise SpecError("py case %d: expr" % i)
        if "expect" in c:
            try:
                ast.literal_eval(c["expect"])
            except (ValueError, SyntaxError, TypeError, MemoryError, RecursionError):
                raise SpecError("py case %d: expect is not a Python literal" % i)
    return cases


def step_py_cases(step: dict, ws: str, cd: str, scratch: str, deadline: float) -> list:
    """All cases in one child process (one interpreter start per step, not per case). The child arms a
    per-case alarm; if it dies or hangs anyway, the case it was on fails and a fresh child resumes with
    the next case, so one bad case never costs the others."""
    if not isinstance(step["import"], str) or not PY_IMPORT.match(step["import"]):
        raise SpecError("py_cases: import must be a dotted module name")
    cases = py_cases_list(step, cd)
    default_t = step.get("timeout_s", 5)
    limits = [c.get("timeout_s", default_t) for c in cases]
    results = [None] * len(cases)
    i = 0
    while i < len(cases):
        job = json.dumps({"ws": ws, "import": step["import"], "exprs": [c["expr"] for c in cases[i:]],
                          "timeouts": limits[i:]}).encode()
        code, raw = spawn(py_child_argv() + ["--child-py-case"], ws, child_env(), job,
                          remaining(deadline, sum(limits[i:]) + 2))
        got = []
        for line in raw.decode("utf-8", "replace").splitlines():
            try:
                got.append(json.loads(line))
            except ValueError:
                break  # only the child's own result lines are well-formed JSON objects in order
        got = got[:len(cases) - i]
        for k, r in enumerate(got):
            results[i + k] = r
        i += len(got)
        if i < len(cases):
            i += 1  # the case the child died or hung on fails; resume after it
        if time.monotonic() >= deadline:
            break
    return [(c["name"], py_case_ok(c, r)) for c, r in zip(cases, results)]


def py_case_ok(case: dict, res: object) -> bool:
    if not isinstance(res, dict):
        return False
    if "raises" in case:
        return res.get("ok") is False and isinstance(res.get("exc"), list) and case["raises"] in res["exc"]
    if res.get("ok") is not True or not isinstance(res.get("repr"), str):
        return False
    try:
        got = ast.literal_eval(res["repr"])
    except (ValueError, SyntaxError, TypeError, MemoryError, RecursionError):
        return False
    approx = case.get("approx")
    return same(got, ast.literal_eval(case["expect"]), float(approx) if approx is not None else None)


def step_py_unittest(step: dict, ws: str, cd: str, scratch: str, deadline: float) -> list:
    names = names_ok(step["names"], "py_unittest names")
    tests = step["tests"]
    if not isinstance(tests, list) or not tests:
        raise SpecError("py_unittest: tests must be a non-empty list of check-dir files")
    paths = [check_file(cd, t) for t in tests]
    job = json.dumps({"ws": ws, "tests": paths}).encode()
    code, raw = spawn(py_child_argv() + ["--child-py-unittest"], ws, child_env(), job,
                      remaining(deadline, step.get("timeout_s", 30)))
    seen = {}
    for line in raw.decode("utf-8", "replace").splitlines():
        try:
            r = json.loads(line)
        except ValueError:
            continue
        if isinstance(r, dict) and isinstance(r.get("id"), str) and isinstance(r.get("ok"), bool):
            seen[r["id"]] = seen.get(r["id"], True) and r["ok"]  # any failing report wins
    return [(n, seen.get(n) is True) for n in names]


def step_cargo_test(step: dict, ws: str, cd: str, scratch: str, deadline: float) -> list:
    names = names_ok(step["names"], "cargo_test names")
    tests = step["tests"]
    if not isinstance(tests, dict) or not tests or not all(
            re.match(r"^[a-z][a-z0-9_]{0,63}\.rs$", k) and isinstance(v, str) for k, v in tests.items()):
        raise SpecError("cargo_test: tests must map <name>.rs to check-dir files")
    manifest = check_file(cd, step["manifest"])
    env = child_env()
    cargo = shutil.which("cargo", path=env.get("PATH", ""))
    if not cargo or not shutil.which("rustc", path=env.get("PATH", "")):
        raise ToolMissing("cargo")
    crate = os.path.join(scratch, "crate")
    skip = shutil.ignore_patterns("target", ".cargo", ".git")
    # copy (not copy2): fresh mtimes, so cargo can never take the copy for an older build of itself
    shutil.copytree(ws, crate, symlinks=True, ignore=skip, copy_function=shutil.copy)
    for name in ("build.rs", "rust-toolchain", "rust-toolchain.toml"):  # crate root only; src/build.rs is a module
        p = os.path.join(crate, name)
        if os.path.lexists(p) and not os.path.isdir(p):
            os.remove(p)
    shutil.copyfile(manifest, os.path.join(crate, "Cargo.toml"))
    os.makedirs(os.path.join(crate, "tests"), exist_ok=True)
    for name, src in sorted(tests.items()):
        dst = os.path.join(crate, "tests", name)
        if os.path.lexists(dst):
            os.remove(dst)
        shutil.copyfile(check_file(cd, src), dst)
    # Never an inherited target dir: artifact names do not depend on the crate's path, so a shared dir would
    # let this check reuse another workspace's build (a stale lib scoring a no-op stub 1000). Each check
    # builds from scratch in its own target dir, deleted with the scratch dir.
    env["CARGO_TARGET_DIR"] = os.path.join(scratch, "target")
    env.update({"CARGO_TERM_COLOR": "never", "CARGO_INCREMENTAL": "0", "CARGO_NET_OFFLINE": "true",
                "CARGO_PROFILE_DEV_DEBUG": "0", "CARGO_PROFILE_TEST_DEBUG": "0"})
    # no --quiet: it would also make libtest print dots instead of the per-test lines parsed below
    argv = [cargo, "test", "--offline", "--manifest-path", os.path.join(crate, "Cargo.toml")]
    for name in sorted(tests):
        argv += ["--test", name[:-3]]
    argv += ["--", "--test-threads=1", "--format=pretty"]
    code, raw = spawn(argv, crate, env, None, remaining(deadline, step.get("timeout_s", 45)))
    status = {}
    for line in raw.decode("utf-8", "replace").splitlines():
        m = CARGO_LINE.match(line)
        if m:
            prev = status.get(m.group(1), "ok")
            status[m.group(1)] = "ok" if (prev == "ok" and m.group(2) == "ok") else "bad"
    return [(n, status.get(n) == "ok") for n in names]


def step_sh(step: dict, ws: str, cd: str, scratch: str, deadline: float) -> list:
    names = names_ok(step["names"], "sh names")
    script = check_file(cd, step["script"])
    env = child_env(drop_check_dir=False)
    env["S1_SCRATCH"] = tempfile.mkdtemp(prefix="sh-", dir=scratch)
    code, raw = spawn(["/bin/sh", script], ws, env, None, remaining(deadline, step.get("timeout_s", 20)))
    verdict = {}
    for line in raw.decode("utf-8", "replace").splitlines():
        m = SH_LINE.match(line)
        if m:
            verdict[m.group(2)] = verdict.get(m.group(2), True) and m.group(1) == "PASS"
    return [(n, verdict.get(n) is True) for n in names]


STEPS = {"files": step_files, "run": step_run, "py_cases": step_py_cases, "py_unittest": step_py_unittest,
         "cargo_test": step_cargo_test, "sh": step_sh}


def aggregate(scoring: str, passed: int, total: int) -> int:
    if total == 0:
        return 0
    if scoring == "all":
        return 1000 if passed == total else 0
    if scoring == "any":
        return 1000 if passed > 0 else 0
    return (1000 * passed) // total


# ------------------------------------------------------------------------------------------ children


def _real_stdout():
    """Keeps a private handle on fd 1 and points fd 1 (and sys.stdout) at /dev/null for the
    untrusted code that follows. Forging a result is pointless: the parent holds the expectation."""
    out = os.fdopen(os.dup(1), "w", encoding="utf-8")
    devnull = os.open(os.devnull, os.O_WRONLY)
    os.dup2(devnull, 1)
    sys.stdout = open(os.devnull, "w")
    return out


def _child_setup(ws: str) -> None:
    sys.path[:] = [p for p in sys.path if p and os.path.abspath(p) != os.path.dirname(os.path.abspath(__file__))]
    sys.path.append(ws)  # last: a workspace file can never shadow the standard library


class _CaseTimeout(BaseException):
    pass


def _alarm(signum, frame):
    raise _CaseTimeout()


def _eval_all(imp: str, exprs: list, emit, timeouts: list | None = None) -> None:
    import importlib
    if timeouts is not None:
        signal.signal(signal.SIGALRM, _alarm)

    def arm(k: int) -> None:
        if timeouts is not None:
            signal.setitimer(signal.ITIMER_REAL, float(timeouts[k]))

    def disarm() -> None:
        if timeouts is not None:
            signal.setitimer(signal.ITIMER_REAL, 0)

    try:
        arm(0)
        ns = dict(vars(importlib.import_module(imp)))
        disarm()
    except BaseException as e:  # noqa: B902 - SystemExit included: an import that exits fails every case
        disarm()
        for _ in exprs:
            emit({"ok": False, "exc": [c.__name__ for c in type(e).__mro__]})
        return
    for k, expr in enumerate(exprs):
        try:
            arm(k)
            val = eval(compile(expr, "<case>", "eval"), ns)  # noqa: S307 - expressions come from the check dir
            r = repr(val)
            disarm()
            emit({"ok": True, "repr": r if len(r) <= CAP_OUT // 4 else ""})
        except BaseException as e:  # noqa: B902
            disarm()
            emit({"ok": False, "exc": [c.__name__ for c in type(e).__mro__]})


def child_py_case() -> None:
    job = json.loads(sys.stdin.read())
    out = _real_stdout()
    _child_setup(job["ws"])

    def emit(r: dict) -> None:
        out.write(json.dumps(r) + "\n")
        out.flush()
    _eval_all(job["import"], job["exprs"], emit, job.get("timeouts"))
    os._exit(0)


def child_py_batch() -> None:
    """Generators only (trusted reference code): one process, many expressions, one JSON list out."""
    job = json.loads(sys.stdin.read())
    out = _real_stdout()
    _child_setup(job["ws"])
    results = []
    _eval_all(job["import"], job["exprs"], results.append)
    out.write(json.dumps(results) + "\n")
    out.flush()
    os._exit(0)


def child_py_unittest() -> None:
    import importlib.util
    import unittest
    job = json.loads(sys.stdin.read())
    out = _real_stdout()
    _child_setup(job["ws"])

    def emit(tid: str, ok: bool) -> None:
        out.write(json.dumps({"id": tid, "ok": ok}) + "\n")
        out.flush()

    class Result(unittest.TestResult):
        def startTest(self, test):  # noqa: N802 - unittest API
            super().startTest(test)
            self._bad = False

        def addError(self, test, err):  # noqa: N802
            self._bad = True

        def addFailure(self, test, err):  # noqa: N802
            self._bad = True

        def addSkip(self, test, reason):  # noqa: N802
            self._bad = True

        def addUnexpectedSuccess(self, test):  # noqa: N802
            self._bad = True

        def addSubTest(self, test, subtest, err):  # noqa: N802
            if err is not None:
                self._bad = True

        def stopTest(self, test):  # noqa: N802
            super().stopTest(test)
            emit(test.id(), not self._bad)

    loader = unittest.TestLoader()
    suite = unittest.TestSuite()
    for path in job["tests"]:
        name = os.path.splitext(os.path.basename(path))[0]
        try:
            spec = importlib.util.spec_from_file_location(name, path)
            mod = importlib.util.module_from_spec(spec)
            sys.modules[name] = mod
            spec.loader.exec_module(mod)
            suite.addTests(loader.loadTestsFromModule(mod))
        except BaseException:  # noqa: B902 - an import error fails that file's tests (they never report)
            continue
    try:
        suite.run(Result())
    except BaseException:  # noqa: B902
        pass
    os._exit(0)


# --------------------------------------------------------------------------------------------- main


def main_check(cd: str) -> None:
    t0 = time.monotonic()
    ws = os.getcwd()
    try:
        spec = load_spec(cd)
        protected = load_protected(cd)
    except SpecError as e:
        log("spec: %s" % e)
        finish(0, code=EXIT_SPEC)
        return
    reasons = tamper_reasons(ws, cd, protected)
    if reasons:
        log("tamper: " + "; ".join(reasons[:8]))
        if spec["on_tamper"] == "score_1000":
            finish(1000)
        finish(0, tamper=True, code=EXIT_TAMPER)
        return
    deadline = t0 + spec["budget_s"]
    passed = total = 0
    scratch = tempfile.mkdtemp(prefix="s1check-")
    try:
        for i, step in enumerate(spec["steps"]):
            w = step.get("weight", 1)
            results = STEPS[step["kind"]](step, ws, cd, scratch, deadline)
            ok = sum(1 for _, r in results if r)
            log("step %d %s %s: %d/%d" % (i, step["kind"], step.get("name", ""), ok, len(results)))
            passed += w * ok
            total += w * len(results)
    except SpecError as e:
        log("spec: %s" % e)
        finish(0, code=EXIT_SPEC)
        return
    except ToolMissing as e:
        log("toolchain_missing: %s" % e)
        finish(0, code=EXIT_TOOL)
        return
    finally:
        shutil.rmtree(scratch, ignore_errors=True)
    finish(aggregate(spec["scoring"], passed, total))


def main(argv: list) -> None:
    modes = {"--child-py-case": child_py_case, "--child-py-unittest": child_py_unittest,
             "--child-py-batch": child_py_batch}
    if len(argv) == 2 and argv[1] in modes:
        modes[argv[1]]()
        return
    if len(argv) < 2 or not os.path.isdir(argv[1]):
        log("usage: s1check.py <check_dir>")
        finish(0, code=EXIT_SPEC)
        return
    main_check(os.path.abspath(argv[1]))


if __name__ == "__main__":
    main(sys.argv)
