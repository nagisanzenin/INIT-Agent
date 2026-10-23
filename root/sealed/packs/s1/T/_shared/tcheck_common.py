#!/usr/bin/env python3
"""INIT pack s1, family T: hidden checker (written fresh for INIT, INIT licence). Python >= 3.9, stdlib only.

Run by hidden/check.sh as `python3 -I -S -B tcheck.py <hidden dir>` with cwd = the workspace. It prints one
`PASS <name>` or `FAIL <name>` line per check name in spec.json `_names`; the shared runner (s1check sh step)
turns them into the score. Workspace code only ever runs inside copies under the runner's scratch dir.
This prelude is shared; the template's own checks follow it and define main().
"""
import hashlib
import json
import os
import shutil
import signal
import stat
import subprocess
import sys
import tempfile
import time

HIDDEN = os.path.abspath(sys.argv[1]) if len(sys.argv) > 1 else os.path.dirname(os.path.abspath(__file__))
WS = os.getcwd()
with open(os.path.join(HIDDEN, "spec.json"), "rb") as _f:
    SPEC = json.loads(_f.read().decode("utf-8"))
NAMES = SPEC["_names"]
_REPORTED = {}
SCRATCH = tempfile.mkdtemp(prefix="tchk-", dir=os.environ.get("S1_SCRATCH") or None)
T_START = time.monotonic()
BUDGET_S = 25.0


def say(msg):
    sys.stderr.write("tcheck: %s\n" % msg)


def report(name, ok, why=""):
    """Records one verdict (the first one for a name wins)."""
    if name not in NAMES:
        raise RuntimeError("unknown check name %s" % name)
    if name in _REPORTED:
        return
    _REPORTED[name] = bool(ok)
    if why and not ok:
        say("%s: %s" % (name, why))


def flush_reports():
    for n in NAMES:
        sys.stdout.write("%s %s\n" % ("PASS" if _REPORTED.get(n) else "FAIL", n))
    sys.stdout.flush()


def run_checks(fns):
    """fns: list of (name or list of names, callable). A callable returns bool, or a dict name->bool for
    several names, and may raise; an exception fails its names."""
    try:
        for names, fn in fns:
            names_l = names if isinstance(names, list) else [names]
            try:
                r = fn()
                if isinstance(r, dict):
                    for n in names_l:
                        report(n, bool(r.get(n)))
                else:
                    for n in names_l:
                        report(n, bool(r))
            except Exception as e:  # noqa: BLE001 - any failure of a check fails it
                for n in names_l:
                    report(n, False, "%s: %s" % (type(e).__name__, e))
    finally:
        flush_reports()
        shutil.rmtree(SCRATCH, ignore_errors=True)


# ------------------------------------------------------------------------------------------- files


def ws_path(rel):
    return os.path.join(WS, rel)


def read_bytes(path, cap=8 << 20):
    """Reads a regular file (not following a final symlink); None when missing or not a file."""
    try:
        st = os.lstat(path)
    except OSError:
        return None
    if not stat.S_ISREG(st.st_mode) or st.st_size > cap:
        return None
    with open(path, "rb") as f:
        return f.read(cap + 1)


def read_text(path):
    b = read_bytes(path)
    if b is None:
        return None
    try:
        return b.decode("utf-8")
    except UnicodeDecodeError:
        return None


def norm_lines(s):
    """Text compare mode: CRLF -> LF, trailing spaces per line kept, trailing empty lines dropped."""
    if s is None:
        return None
    ls = s.replace("\r\n", "\n").split("\n")
    while ls and ls[-1] == "":
        ls.pop()
    return ls


def sha256_file(path):
    b = read_bytes(path, cap=256 << 20)
    return None if b is None else hashlib.sha256(b).hexdigest()


def tree_listing(root, skip=()):
    """{relative path: ('f', sha256, executable) | ('d',) | ('l', target)} without following links."""
    out = {}
    for dirpath, dirnames, filenames in os.walk(root):
        rel_dir = os.path.relpath(dirpath, root)
        for name in sorted(dirnames + filenames):
            p = os.path.join(dirpath, name)
            rel = name if rel_dir == "." else rel_dir.replace(os.sep, "/") + "/" + name
            if any(rel == s or rel.startswith(s + "/") for s in skip):
                continue
            st = os.lstat(p)
            if stat.S_ISLNK(st.st_mode):
                out[rel] = ("l", os.readlink(p))
            elif stat.S_ISDIR(st.st_mode):
                out[rel] = ("d",)
            elif stat.S_ISREG(st.st_mode):
                with open(p, "rb") as f:
                    out[rel] = ("f", hashlib.sha256(f.read()).hexdigest(), bool(st.st_mode & 0o111))
            else:
                out[rel] = ("o",)
        dirnames[:] = [d for d in dirnames if not os.path.islink(os.path.join(dirpath, d))]
    return out


def copy_ws(name="ws", skip=()):
    """A private copy of the workspace (symlinks copied as links) under the scratch dir."""
    dst = os.path.join(SCRATCH, name)
    ign = shutil.ignore_patterns(*skip) if skip else None
    shutil.copytree(WS, dst, symlinks=True, ignore=ign)
    return dst


def write_file(path, data, mode=0o644):
    os.makedirs(os.path.dirname(path) or ".", exist_ok=True)
    with open(path, "wb") as f:
        f.write(data if isinstance(data, bytes) else data.encode("utf-8"))
    os.chmod(path, mode)


# ------------------------------------------------------------------------------------------ processes


def base_env(extra=None):
    home = os.path.join(SCRATCH, "home")
    tmp = os.path.join(SCRATCH, "tmp")
    os.makedirs(home, exist_ok=True)
    os.makedirs(tmp, exist_ok=True)
    env = {"PATH": os.environ.get("PATH") or "/usr/bin:/bin:/usr/sbin:/sbin", "HOME": home, "TMPDIR": tmp,
           "LANG": "C", "LC_ALL": "C", "TZ": "UTC", "PYTHONDONTWRITEBYTECODE": "1", "PYTHONHASHSEED": "0",
           "GIT_CONFIG_NOSYSTEM": "1", "GIT_TERMINAL_PROMPT": "0", "MAKEFLAGS": "", "MFLAGS": ""}
    for k in ("DEVELOPER_DIR", "SDKROOT"):
        if os.environ.get(k):
            env[k] = os.environ[k]
    if extra:
        env.update(extra)
    return env


def run(argv, cwd, stdin=b"", timeout=5.0, env=None):
    """Runs argv in its own process group; returns (exit code or None on timeout, stdout, stderr)."""
    left = BUDGET_S - (time.monotonic() - T_START)
    timeout = max(0.1, min(timeout, left))
    if isinstance(stdin, str):
        stdin = stdin.encode("utf-8")
    try:
        p = subprocess.Popen(argv, cwd=cwd, env=env if env is not None else base_env(), stdin=subprocess.PIPE,
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    except OSError as e:
        return -1, b"", str(e).encode()
    try:
        out, err = p.communicate(stdin, timeout=timeout)
        return p.returncode, out, err
    except subprocess.TimeoutExpired:
        try:
            os.killpg(p.pid, signal.SIGKILL)
        except OSError:
            pass
        try:
            out, err = p.communicate(timeout=2)
        except subprocess.TimeoutExpired:
            out, err = b"", b""
        return None, out, err


def git(args, cwd, timeout=5.0):
    """git with no system/global config and any repo owner accepted; returns (code, stdout text)."""
    env = base_env({"GIT_CONFIG_GLOBAL": os.path.join(SCRATCH, "gitconfig-empty")})
    write_file(os.path.join(SCRATCH, "gitconfig-empty"), "")
    code, out, _ = run(["git", "-c", "safe.directory=*", "-c", "core.fsmonitor=false"] + list(args), cwd,
                       timeout=timeout, env=env)
    return code, out.decode("utf-8", "replace")


def is_exec_file(path):
    try:
        st = os.stat(path)
    except OSError:
        return False
    return stat.S_ISREG(st.st_mode) and bool(st.st_mode & 0o100)


def tool_argv(path):
    """argv to run a workspace tool: executable files run directly; a non-executable script runs through
    its shebang interpreter is NOT attempted (the task says the tool must be executable)."""
    return [path]
