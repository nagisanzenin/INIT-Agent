# t-shell-quoting: run the workspace script with /bin/sh and a restricted PATH on hidden file-name sets, each in
# its own private directory, and compare the whole resulting tree (and stdout where the shape prints).
import re as _re

FORBIDDEN = _re.compile(r"\b(python[0-9.]*|perl[0-9.]*|ruby|node|php|osascript|swift|tclsh|lua|jq)\b")
_TOOLS = {}


def tools_dir():
    """A PATH directory holding links to the allowed utilities only."""
    if "d" not in _TOOLS:
        d = os.path.join(SCRATCH, "tools")
        os.makedirs(d)
        for t in SPEC["tools"]:
            for base in ("/bin", "/usr/bin"):
                p = os.path.join(base, t)
                if os.path.isfile(p) and os.access(p, os.X_OK):
                    os.symlink(p, os.path.join(d, t))
                    break
        _TOOLS["d"] = d
    return _TOOLS["d"]


def snapshot(root):
    """{relative path: text} of every file below root; links and special or huge files map to None."""
    out = {}
    n = 0
    for dirpath, dirnames, filenames in os.walk(root):
        for name in filenames + [d for d in dirnames if os.path.islink(os.path.join(dirpath, d))]:
            n += 1
            if n > 2000:
                out["<too many files>"] = None
                return out
            p = os.path.join(dirpath, name)
            rel = os.path.relpath(p, root)
            st = os.lstat(p)
            if stat.S_ISREG(st.st_mode) and st.st_size <= (1 << 20):
                with open(p, "rb") as f:
                    out[rel] = f.read().decode("utf-8", "replace")
            else:
                out[rel] = None
        dirnames[:] = [d for d in dirnames if not os.path.islink(os.path.join(dirpath, d))]
    return out


def script_copy():
    """The workspace script copied into a private directory (None when missing, a link, or not text)."""
    if "s" not in _TOOLS:
        data = read_bytes(ws_path(SPEC["script"]), cap=1 << 20)
        path = None
        if data is not None:
            try:
                text = data.decode("utf-8")
            except UnicodeDecodeError:
                text = None
            if text is None:
                say("script is not UTF-8 text")
            elif FORBIDDEN.search(text):
                say("the script calls another interpreter")
            else:
                d = os.path.join(SCRATCH, "script")
                os.makedirs(d)
                path = os.path.join(d, SPEC["script"])
                write_file(path, data, 0o755)
        else:
            say("%s missing, a link or too big" % SPEC["script"])
        _TOOLS["s"] = path
    return _TOOLS["s"]


def scenario(case):
    def fn():
        script = script_copy()
        if script is None:
            return False
        rd = os.path.join(SCRATCH, "run-" + case["name"])
        os.makedirs(rd)
        for rel in case["dirs"]:
            os.makedirs(os.path.join(rd, rel), exist_ok=True)
        for rel, data in sorted(case["files"].items()):
            write_file(os.path.join(rd, rel), data)
        code, out, _err = run(["/bin/sh", script] + case["args"], rd, timeout=6,
                              env=base_env({"PATH": tools_dir()}))
        if code is None:
            say("%s: timed out" % case["name"])
            return False
        got = snapshot(rd)
        want = case["want"]
        if got != want:
            diff = [k for k in sorted(set(got) | set(want)) if got.get(k) != want.get(k)]
            say("%s: %d path(s) differ, e.g. %r" % (case["name"], len(diff), diff[:3]))
            return False
        if case["stdout"] is not None and out.decode("utf-8", "replace") != case["stdout"]:
            say("%s: stdout differs" % case["name"])
            return False
        return True
    return fn


def main():
    run_checks([(c["name"], scenario(c)) for c in SPEC["cases"]])
