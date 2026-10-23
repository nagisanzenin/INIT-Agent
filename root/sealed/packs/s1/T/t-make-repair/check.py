# t-make-repair: build scenarios in private copies of the workspace with explicit, deterministic mtimes.

T0 = 1700000000


def make(cwd, *args):
    code, out, err = run(["make"] + list(args), cwd, timeout=8)
    return code, out.decode("utf-8", "replace"), err.decode("utf-8", "replace")


def fresh_copy(tag):
    d = copy_ws(tag, skip=(".git",))
    shutil.rmtree(os.path.join(d, "build"), ignore_errors=True)
    if os.path.lexists(os.path.join(d, "build")):
        os.remove(os.path.join(d, "build"))
    return d


def book_path(d):
    return os.path.join(d, "build", SPEC["book"] + ".txt")


def build_ok(d):
    code, out, err = make(d)
    if code != 0:
        say("make failed (%s): %s" % (code, err[-400:]))
        return False
    return read_text(book_path(d)) == SPEC["want_fresh"]


def settle(d):
    """Sources at T0, build outputs (and build/) at T0+10: a correct Makefile then has nothing to do."""
    for dirpath, dirnames, filenames in os.walk(d):
        for name in filenames + dirnames:
            p = os.path.join(dirpath, name)
            if os.path.islink(p):
                continue
            rel = os.path.relpath(p, d)
            t = T0 + 10 if (rel == "build" or rel.startswith("build" + os.sep)) else T0
            os.utime(p, (t, t))


def build_stamps(d):
    out = {}
    root = os.path.join(d, "build")
    for dirpath, _, filenames in os.walk(root):
        for name in filenames:
            p = os.path.join(dirpath, name)
            st = os.lstat(p)
            out[os.path.relpath(p, d)] = (st.st_mtime_ns, st.st_size)
    return out


def touch_write(path, text, t):
    write_file(path, text)
    os.utime(path, (t, t))


_BASE = {}


def base():
    """One fresh build, settled; None when the fresh build fails. Scenarios run on clones of it."""
    if "d" not in _BASE:
        d = fresh_copy("base")
        _BASE["ok"] = build_ok(d)
        if _BASE["ok"]:
            settle(d)
        _BASE["d"] = d
    return _BASE["d"] if _BASE["ok"] else None


def clone(tag):
    src = base()
    if src is None:
        return None
    dst = os.path.join(SCRATCH, tag)
    shutil.copytree(src, dst, symlinks=True)  # copy2 + copystat keep the settled mtimes
    return dst


def scenario(tag, change, want):
    def fn():
        d = clone(tag)
        if d is None:
            return False
        change(d)
        code, _, err = make(d)
        if code != 0:
            say("%s: make failed: %s" % (tag, err[-300:]))
            return False
        got = read_text(book_path(d))
        if got != want:
            say("%s: bundle differs" % tag)
        return got == want
    return fn


def c_up_to_date():
    d = clone("u")
    if d is None:
        return False
    before = build_stamps(d)
    code, _, _ = make(d)
    after = build_stamps(d)
    if code != 0 or before != after:
        say("up_to_date: a second make changed %s" % sorted(set(before.items()) ^ set(after.items()))[:4])
        return False
    return True


def c_edit(d):
    touch_write(os.path.join(d, SPEC["edit"]["path"]), SPEC["edit"]["text"], T0 + 20)


def c_add(d):
    touch_write(os.path.join(d, SPEC["add"]["path"]), SPEC["add"]["text"], T0 + 20)


def c_tool(d):
    p = os.path.join(d, "tools", "render.sh")
    src = read_text(p) or ""
    touch_write(p, src.replace(SPEC["tool"]["old"], SPEC["tool"]["new"]), T0 + 20)


def c_clean():
    d = clone("c")
    if d is None:
        return False
    code, _, _ = make(d, "clean")
    pages_left = os.path.isdir(os.path.join(d, "pages")) and bool(os.listdir(os.path.join(d, "pages")))
    return code == 0 and not os.path.lexists(os.path.join(d, "build")) and pages_left


def c_test():
    d = clone("t")
    if d is None:
        return False
    code, out, _ = make(d, "test")
    return code == 0 and "test/run.sh: ran" in out and "test: OK" in out


def main():
    run_checks([
        ("fresh_build", lambda: base() is not None),
        ("up_to_date", c_up_to_date),
        ("incremental_edit", scenario("e", c_edit, SPEC["edit"]["want"])),
        ("new_page", scenario("a", c_add, SPEC["add"]["want"])),
        ("tool_change", scenario("r", c_tool, SPEC["tool"]["want"])),
        ("clean", c_clean),
        ("test_target", c_test),
    ])
