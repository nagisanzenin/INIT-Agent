#!/usr/bin/env python3
"""poolcheck: the check runner of the visible maintainer pool v1 (docs/m7-design.md §3.3). Python >= 3.9, stdlib.

Every pool task's `check/run` is a small sh wrapper that execs

    python3 -I -S -B <check_dir>/poolcheck.py <check_dir>

with cwd = the workspace (kernel/gate.md §3.7.1: OUT/ws, INIT_CHECK_DIR = OUT/check). It reads
<check_dir>/check.json (what to run) and <check_dir>/protected.json (visible tests and fixtures that must not
change).

This is the pool's own code, not a copy of the sealed pack's runner: root/sealed/packs/s1/FORMAT.md keeps pack
files out of fixtures/ and INIT_HOME. It follows the same contract for the step kinds the pool uses, so a
pool check.json reads like an s1 one: same keys, same scoring, same exit codes, same tamper line.

Output: the LAST stdout line is the score line (kernel/gate.md §3.7.1, v3 grammar):
  exit 0  {"score_permille":N}
  exit 3  {"score_permille":0,"tamper":true}   a protected file changed, is missing or reached through a link,
                                               or a workspace link points into the check dir
  exit 2  {"score_permille":0}                 check.json or protected.json malformed (a pool bug)
  exit 4  {"score_permille":0}                 a toolchain the check needs is missing

check.json: {"v": 1, "runner": "poolcheck/1", "scoring": "fraction"|"all"|"any", "on_tamper"?: "zero",
             "budget_s"?: 50, "steps": [...]}; every step takes optional "name" and "weight" (default 1):
  py_unittest  tests: [check-dir files], names: ["<stem>.<Class>.<method>"], timeout_s? (30)
               a named test ran and passed (a failing subTest fails it)
  py_cases     import: dotted module (workspace last on sys.path), cases: check-dir JSON file or inline list of
               {name, expr, expect (a Python literal) | raises (exception class or base name), approx?,
               timeout_s?}; timeout_s? (5 per case). repr(eval(expr)) crosses back and is compared here with
               equal types (True is not 1)
  cargo_test   manifest: check-dir file (replaces Cargo.toml), tests: {"<name>.rs": check-dir file},
               names: [test fn names], timeout_s? (45). The crate is copied with fresh mtimes (target, .cargo,
               .git, build.rs, rust-toolchain* dropped) and built in a fresh target dir, never an inherited one
  sh           script: check-dir file run by /bin/sh, cwd = workspace, $S1_SCRATCH (a fresh dir) and
               $INIT_CHECK_DIR set; names: [...]; timeout_s? (20). A name passes when the script printed
               "PASS <name>" and no "FAIL <name>"
Scoring: fraction = floor(1000 x passed weight / total weight); all = 1000 iff all pass; any = 1000 iff one.
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
import time

RUNNER_ID = "poolcheck/1"
EXIT_OK, EXIT_SPEC, EXIT_TAMPER, EXIT_TOOL = 0, 2, 3, 4
CAP_OUT = 4 << 20
DEFAULT_BUDGET_S = 50
NAME = re.compile(r"^[A-Za-z0-9_.:/-]{1,200}$")
PY_IMPORT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*(\.[A-Za-z_][A-Za-z0-9_]*)*$")
CARGO_LINE = re.compile(r"^test (\S+) \.\.\. (ok|FAILED|ignored)\s*$")
SH_LINE = re.compile(r"^(PASS|FAIL) (\S+)\s*$")
TOP_KEYS = ({"v", "runner", "scoring", "steps"}, {"on_tamper", "budget_s"})
STEP_KEYS = {
    "py_unittest": ({"kind", "tests", "names"}, {"name", "weight", "timeout_s"}),
    "py_cases": ({"kind", "import", "cases"}, {"name", "weight", "timeout_s"}),
    "cargo_test": ({"kind", "manifest", "tests", "names"}, {"name", "weight", "timeout_s"}),
    "sh": ({"kind", "script", "names"}, {"name", "weight", "timeout_s"}),
}


class SpecError(Exception):
    pass


class ToolMissing(Exception):
    pass


def log(msg: str) -> None:
    sys.stderr.write("poolcheck: %s\n" % msg)


def finish(score: int, tamper: bool = False, code: int = EXIT_OK) -> None:
    sys.stdout.write('{"score_permille":0,"tamper":true}\n' if tamper else '{"score_permille":%d}\n' % score)
    sys.stdout.flush()
    sys.exit(code)


# ------------------------------------------------------------------------------------------- spec


def rel_ok(p: object) -> bool:
    if not isinstance(p, str) or not p or len(p) > 512 or "\0" in p or "\\" in p or p.startswith("/"):
        return False
    return all(part not in ("", ".", "..") for part in p.split("/"))


def load_json(path: str) -> object:
    try:
        with open(path, "rb") as f:
            return json.loads(f.read().decode("utf-8"))
    except (OSError, ValueError) as e:
        raise SpecError("%s: %s" % (os.path.basename(path), e))


def check_file(cd: str, rel: object) -> str:
    if not rel_ok(rel):
        raise SpecError("bad check-dir path %r" % (rel,))
    p = os.path.join(cd, rel)
    if not os.path.isfile(p):
        raise SpecError("missing check-dir file %s" % rel)
    return p


def keys_ok(obj: object, required: set, optional: set, what: str) -> dict:
    if not isinstance(obj, dict):
        raise SpecError("%s: not an object" % what)
    missing, extra = required - set(obj), set(obj) - required - optional
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
    spec = keys_ok(load_json(os.path.join(cd, "check.json")), TOP_KEYS[0], TOP_KEYS[1], "check.json")
    if spec["v"] != 1 or spec["runner"] != RUNNER_ID:
        raise SpecError("check.json: v 1 and runner %s expected" % RUNNER_ID)
    if spec["scoring"] not in ("fraction", "all", "any"):
        raise SpecError("check.json: scoring must be fraction, all or any")
    if spec.get("on_tamper", "zero") != "zero":
        raise SpecError("check.json: on_tamper must be zero (no canaries in the pool)")
    spec["budget_s"] = int_in(spec.get("budget_s", DEFAULT_BUDGET_S), 1, 1700, "budget_s")
    if not isinstance(spec["steps"], list) or not spec["steps"]:
        raise SpecError("check.json: steps must be a non-empty list")
    for i, st in enumerate(spec["steps"]):
        kind = st.get("kind") if isinstance(st, dict) else None
        if kind not in STEP_KEYS:
            raise SpecError("step %d: unknown kind %r" % (i, kind))
        keys_ok(st, STEP_KEYS[kind][0], STEP_KEYS[kind][1], "step %d (%s)" % (i, kind))
        int_in(st.get("weight", 1), 1, 1000, "step %d weight" % i)
        int_in(st.get("timeout_s", 1), 1, 600, "step %d timeout_s" % i)
    return spec


def load_protected(cd: str) -> list:
    p = os.path.join(cd, "protected.json")
    if not os.path.exists(p):
        return []
    prot = load_json(p)
    if not isinstance(prot, list):
        raise SpecError("protected.json: list expected")
    for e in prot:
        keys_ok(e, {"path", "sha256"}, set(), "protected.json entry")
        if not rel_ok(e["path"]) or not re.match(r"^[0-9a-f]{64}$", str(e["sha256"])):
            raise SpecError("protected.json: bad entry %r" % (e,))
    return prot


# ------------------------------------------------------------------------------------------ tamper


def read_nofollow(root: str, rel: str) -> bytes | None:
    """root/rel's bytes, or None when missing, not a regular file, or reached through a link."""
    cur = root
    parts = rel.split("/")
    st = None
    for i, part in enumerate(parts):
        cur = os.path.join(cur, part)
        try:
            st = os.lstat(cur)
        except OSError:
            return None
        if stat.S_ISLNK(st.st_mode):
            return None
        if i < len(parts) - 1 and not stat.S_ISDIR(st.st_mode):
            return None
    if st is None or not stat.S_ISREG(st.st_mode):
        return None
    try:
        fd = os.open(cur, os.O_RDONLY | getattr(os, "O_NOFOLLOW", 0))
    except OSError:
        return None
    with os.fdopen(fd, "rb") as f:
        return f.read()


def links_into(ws: str, cd: str) -> list:
    """Workspace symlinks that resolve into the check dir (only a check-time reader plants one)."""
    real_cd = os.path.realpath(cd)
    hits = []
    n = 0
    for dirpath, dirnames, filenames in os.walk(ws):
        for name in dirnames + filenames:
            n += 1
            if n > 200_000:
                return hits
            p = os.path.join(dirpath, name)
            if os.path.islink(p):
                t = os.path.realpath(p)
                if t == real_cd or t.startswith(real_cd + os.sep):
                    hits.append(os.path.relpath(p, ws))
    return hits


def tamper_reasons(ws: str, cd: str, prot: list) -> list:
    bad = []
    for e in prot:
        data = read_nofollow(ws, e["path"])
        if data is None or hashlib.sha256(data).hexdigest() != e["sha256"]:
            bad.append("protected %s changed" % e["path"])
    bad += ["link into the check dir: %s" % p for p in links_into(ws, cd)]
    return bad


# ---------------------------------------------------------------------------------------- children


def child_env(tmp: str, keep_check_dir: bool = False) -> dict:
    """The check's environment (env_clear'ed by the kernel or Root) minus Python and Cargo knobs, plus a fixed
    locale and hash seed, as the s1 runner does; TMPDIR is this step's own scratch dir."""
    env = {}
    for k, v in os.environ.items():
        if k.startswith(("PYTHON", "RUSTC", "RUSTFLAGS", "RUSTDOC", "CARGO_BUILD", "CARGO_ENCODED", "CARGO_TARGET_DIR")):
            continue
        if k == "INIT_CHECK_DIR" and not keep_check_dir:
            continue
        env[k] = v
    env.setdefault("PATH", "/usr/bin:/bin")
    env.update({"TMPDIR": tmp, "LC_ALL": "C", "PYTHONHASHSEED": "0", "PYTHONDONTWRITEBYTECODE": "1",
                "PYTHONNOUSERSITE": "1", "PYTHONIOENCODING": "utf-8"})
    return env


def spawn(argv: list, cwd: str, env: dict, stdin: bytes | None, timeout: float) -> tuple:
    """(exit code or None on timeout, stdout bytes). The child runs in its own process group, which is
    killed on timeout (only processes this runner spawned)."""
    try:
        p = subprocess.Popen(argv, cwd=cwd, env=env, stdin=subprocess.PIPE if stdin is not None else subprocess.DEVNULL,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    except OSError as e:
        raise ToolMissing("%s: %s" % (argv[0], e))
    try:
        out, err = p.communicate(stdin, timeout=max(0.5, timeout))
        code = p.returncode
    except subprocess.TimeoutExpired:
        try:
            os.killpg(p.pid, signal.SIGKILL)
        except OSError:
            pass
        out, err = p.communicate()
        code = None
        log("%s: timeout after %.0f s" % (os.path.basename(argv[0]), timeout))
    if err:
        sys.stderr.write(err.decode("utf-8", "replace")[-3000:])
    return code, out[:CAP_OUT]


def remaining(deadline: float, cap: float) -> float:
    return max(0.5, min(cap, deadline - time.monotonic()))


def py_child_argv() -> list:
    # no -I/-E: the env is built by child_env (PYTHONHASHSEED must apply); -S -s: no site dirs; -B: no bytecode
    return [sys.executable, "-S", "-s", "-B", os.path.abspath(__file__)]


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
    if isinstance(a, (set, frozenset)):
        return a == b
    return a == b


# ------------------------------------------------------------------------------------------- steps


def step_py_unittest(step: dict, ws: str, cd: str, tmp: str, deadline: float) -> list:
    names = names_ok(step["names"], "py_unittest names")
    tests = step["tests"]
    if not isinstance(tests, list) or not tests:
        raise SpecError("py_unittest: tests must be a non-empty list of check-dir files")
    job = json.dumps({"ws": ws, "tests": [check_file(cd, t) for t in tests]}).encode()
    code, raw = spawn(py_child_argv() + ["--child-py-unittest"], ws, child_env(tmp), job,
                      remaining(deadline, step.get("timeout_s", 30)))
    seen = {}
    for line in raw.decode("utf-8", "replace").splitlines():
        try:
            r = json.loads(line)
        except ValueError:
            continue
        if isinstance(r, dict) and isinstance(r.get("id"), str) and isinstance(r.get("ok"), bool):
            seen[r["id"]] = seen.get(r["id"], True) and r["ok"]
    return [(n, seen.get(n) is True) for n in names]


def py_cases_list(step: dict, cd: str) -> list:
    cases = step["cases"]
    if isinstance(cases, str):
        cases = load_json(check_file(cd, cases))
    if not isinstance(cases, list) or not cases:
        raise SpecError("py_cases: cases must be a non-empty list")
    seen = set()
    for i, c in enumerate(cases):
        keys_ok(c, {"name", "expr"}, {"expect", "raises", "approx", "timeout_s"}, "py case %d" % i)
        if ("expect" in c) == ("raises" in c):
            raise SpecError("py case %d: exactly one of expect/raises" % i)
        if not isinstance(c["name"], str) or not NAME.match(c["name"]) or c["name"] in seen:
            raise SpecError("py case %d: bad or duplicate name" % i)
        seen.add(c["name"])
        if "expect" in c:
            try:
                ast.literal_eval(c["expect"])
            except (ValueError, SyntaxError, TypeError, MemoryError, RecursionError):
                raise SpecError("py case %d: expect is not a Python literal" % i)
    return cases


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


def step_py_cases(step: dict, ws: str, cd: str, tmp: str, deadline: float) -> list:
    """All cases in one child; a per-case alarm in the child, and a fresh child resumes after a case that
    killed or hung the previous one."""
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
        code, raw = spawn(py_child_argv() + ["--child-py-cases"], ws, child_env(tmp), job,
                          remaining(deadline, sum(limits[i:]) + 2))
        got = []
        for line in raw.decode("utf-8", "replace").splitlines():
            try:
                got.append(json.loads(line))
            except ValueError:
                break
        got = got[:len(cases) - i]
        for k, r in enumerate(got):
            results[i + k] = r
        i += len(got)
        if i < len(cases):
            i += 1
        if time.monotonic() >= deadline:
            break
    return [(c["name"], py_case_ok(c, r)) for c, r in zip(cases, results)]


def step_cargo_test(step: dict, ws: str, cd: str, tmp: str, deadline: float) -> list:
    names = names_ok(step["names"], "cargo_test names")
    tests = step["tests"]
    if not isinstance(tests, dict) or not tests or not all(
            re.match(r"^[a-z][a-z0-9_]{0,63}\.rs$", k) and isinstance(v, str) for k, v in tests.items()):
        raise SpecError("cargo_test: tests must map <name>.rs to check-dir files")
    manifest = check_file(cd, step["manifest"])
    env = child_env(tmp)
    cargo = shutil.which("cargo", path=env["PATH"])
    if not cargo or not shutil.which("rustc", path=env["PATH"]):
        raise ToolMissing("cargo")
    crate = os.path.join(tmp, "crate")
    skip = shutil.ignore_patterns("target", ".cargo", ".git", "rust-toolchain", "rust-toolchain.toml", "build.rs")
    # copy (not copy2): fresh mtimes, so cargo never mistakes the copy for an older build of itself
    shutil.copytree(ws, crate, symlinks=True, ignore=skip, copy_function=shutil.copy)
    shutil.copyfile(manifest, os.path.join(crate, "Cargo.toml"))
    os.makedirs(os.path.join(crate, "tests"), exist_ok=True)
    for name, src in sorted(tests.items()):
        dst = os.path.join(crate, "tests", name)
        if os.path.lexists(dst):
            os.remove(dst)
        shutil.copyfile(check_file(cd, src), dst)
    # never an inherited target dir: a shared one lets this check reuse another workspace's build
    env["CARGO_TARGET_DIR"] = os.path.join(tmp, "target")
    env.update({"CARGO_TERM_COLOR": "never", "CARGO_INCREMENTAL": "0", "CARGO_NET_OFFLINE": "true",
                "CARGO_PROFILE_DEV_DEBUG": "0", "CARGO_PROFILE_TEST_DEBUG": "0"})
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


def step_sh(step: dict, ws: str, cd: str, tmp: str, deadline: float) -> list:
    names = names_ok(step["names"], "sh names")
    script = check_file(cd, step["script"])
    env = child_env(tmp, keep_check_dir=True)
    env["S1_SCRATCH"] = tempfile.mkdtemp(prefix="sh-", dir=tmp)
    code, raw = spawn(["/bin/sh", script], ws, env, None, remaining(deadline, step.get("timeout_s", 20)))
    verdict = {}
    for line in raw.decode("utf-8", "replace").splitlines():
        m = SH_LINE.match(line)
        if m:
            verdict[m.group(2)] = verdict.get(m.group(2), True) and m.group(1) == "PASS"
    return [(n, verdict.get(n) is True) for n in names]


STEPS = {"py_unittest": step_py_unittest, "py_cases": step_py_cases, "cargo_test": step_cargo_test, "sh": step_sh}


def aggregate(scoring: str, passed: int, total: int) -> int:
    if total == 0:
        return 0
    if scoring == "all":
        return 1000 if passed == total else 0
    if scoring == "any":
        return 1000 if passed > 0 else 0
    return (1000 * passed) // total


def main_check(cd: str) -> None:
    ws = os.getcwd()
    try:
        spec = load_spec(cd)
        prot = load_protected(cd)
    except SpecError as e:
        log(str(e))
        finish(0, code=EXIT_SPEC)
    bad = tamper_reasons(ws, cd, prot)
    if bad:
        log("tamper: " + "; ".join(bad))
        finish(0, tamper=True, code=EXIT_TAMPER)
    deadline = time.monotonic() + spec["budget_s"]
    passed = total = 0
    base_tmp = os.environ.get("TMPDIR") or None
    try:
        for i, st in enumerate(spec["steps"]):
            w = st.get("weight", 1)
            tmp = tempfile.mkdtemp(prefix="poolcheck-", dir=base_tmp)
            try:
                if time.monotonic() >= deadline:
                    res = [(n, False) for n in (st.get("names") or ["?"])]
                else:
                    res = STEPS[st["kind"]](st, ws, cd, tmp, deadline)
            finally:
                shutil.rmtree(tmp, ignore_errors=True)
            for n, ok in res:
                total += w
                passed += w if ok else 0
                if not ok:
                    log("step %d (%s): %s failed" % (i, st.get("name", st["kind"]), n))
    except SpecError as e:
        log(str(e))
        finish(0, code=EXIT_SPEC)
    except ToolMissing as e:
        log("toolchain_missing: %s" % e)
        finish(0, code=EXIT_TOOL)
    finish(aggregate(spec["scoring"], passed, total))


# ---------------------------------------------------------------------------------- child modes


def _real_stdout():
    """A private handle on fd 1; fd 1 and sys.stdout then point at /dev/null for the untrusted code."""
    fd = os.dup(1)
    null = os.open(os.devnull, os.O_WRONLY)
    os.dup2(null, 1)
    os.close(null)
    sys.stdout = open(os.devnull, "w")
    return os.fdopen(fd, "w")


def _child_setup(ws: str) -> None:
    here = os.path.dirname(os.path.abspath(__file__))
    sys.path[:] = [p for p in sys.path if p and os.path.abspath(p) != here]
    sys.path.append(ws)  # last: a workspace file can never shadow the standard library
    sys.dont_write_bytecode = True


def _alarm(signum, frame):
    raise TimeoutError("case timeout")


def child_py_cases() -> None:
    import importlib
    job = json.loads(sys.stdin.read())
    out = _real_stdout()
    _child_setup(job["ws"])

    def emit(r: dict) -> None:
        out.write(json.dumps(r, sort_keys=True) + "\n")
        out.flush()

    signal.signal(signal.SIGALRM, _alarm)
    try:
        signal.setitimer(signal.ITIMER_REAL, float(job["timeouts"][0]) if job["timeouts"] else 5.0)
        mod = importlib.import_module(job["import"])
        signal.setitimer(signal.ITIMER_REAL, 0)
    except BaseException as e:  # noqa: B902 - an import failure fails every case
        signal.setitimer(signal.ITIMER_REAL, 0)
        for _ in job["exprs"]:
            emit({"ok": False, "exc": [c.__name__ for c in type(e).__mro__]})
        os._exit(0)
    for expr, t in zip(job["exprs"], job["timeouts"]):
        ns = dict(vars(mod))
        try:
            signal.setitimer(signal.ITIMER_REAL, float(t))
            v = eval(expr, ns)  # noqa: S307 - maintainer-written expressions
            signal.setitimer(signal.ITIMER_REAL, 0)
            emit({"ok": True, "repr": repr(v)})
        except BaseException as e:  # noqa: B902
            signal.setitimer(signal.ITIMER_REAL, 0)
            emit({"ok": False, "exc": [c.__name__ for c in type(e).__mro__]})
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


def main(argv: list) -> None:
    if len(argv) == 2 and argv[1] == "--child-py-cases":
        child_py_cases()
    elif len(argv) == 2 and argv[1] == "--child-py-unittest":
        child_py_unittest()
    elif len(argv) == 2:
        main_check(os.path.abspath(argv[1]))
    else:
        log("usage: poolcheck.py <check_dir>")
        finish(0, code=EXIT_SPEC)


if __name__ == "__main__":
    main(sys.argv)
