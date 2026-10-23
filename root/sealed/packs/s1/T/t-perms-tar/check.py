# t-perms-tar: inspect the agent's own dist tarball, then run `sh package.sh` in a private copy with hidden extra
# files (arbitrary modes) and a changed VERSION, and inspect the archive it builds with tarfile.
import tarfile
import zlib

LEVEL = SPEC["level"]
ROOTS = ("bin", "lib", "conf", "docs")
CAP_ARCHIVE = 64 << 20
CAP_DATA = 32 << 20
CAP_ENTRIES = 5000


def excluded(rel):
    parts = rel.split("/")
    base = parts[-1]
    if base == ".DS_Store" or base.endswith(".tmp"):
        return True
    return LEVEL >= 2 and (base.endswith(".pyc") or "__pycache__" in parts)


def mode_for(rel, kind):
    if kind == "d":
        return 0o755
    parts = rel.split("/")
    if parts[0] == "bin" and len(parts) == 2:
        return 0o755
    if LEVEL >= 3 and parts[0] == "conf" and parts[-1].endswith(".secret"):
        return 0o600
    return 0o644


def inspect(path):
    """[(name, kind, mode, uid, gid, mtime, data)] of a .tar.gz, or None when missing, a link or unreadable."""
    try:
        st = os.lstat(path)
    except OSError:
        say("%s: missing" % os.path.basename(path))
        return None
    if not stat.S_ISREG(st.st_mode) or st.st_size > CAP_ARCHIVE:
        say("%s: not a regular file or too big" % os.path.basename(path))
        return None
    entries = []
    total = 0
    try:
        with tarfile.open(path, "r:gz") as tf:
            for m in tf:
                if len(entries) >= CAP_ENTRIES:
                    return None
                kind = "d" if m.isdir() else ("f" if m.isreg() else "o")
                data = None
                if kind == "f":
                    total += m.size
                    if total > CAP_DATA:
                        return None
                    f = tf.extractfile(m)
                    data = f.read() if f is not None else None
                entries.append((m.name.rstrip("/") if m.name != "/" else m.name, kind, m.mode & 0o7777, m.uid, m.gid,
                                m.mtime, data))
    except (tarfile.TarError, OSError, EOFError, zlib.error, ValueError) as e:
        say("%s: unreadable archive (%s)" % (os.path.basename(path), type(e).__name__))
        return None
    return entries


def judge(entries, want, top, junk_ok=True):
    """name -> bool for every rule (the applicable ones are reported)."""
    r = {}
    if not entries:
        return r
    names = [e[0] for e in entries]
    under = [e for e in entries if e[0] == top or e[0].startswith(top + "/")]
    inner = [e for e in under if e[0] != top]
    r["top_dir"] = len(under) == len(entries) and any(e[0] == top and e[1] == "d" for e in entries)
    got = {}
    for e in entries:
        got.setdefault(e[0], e)
    r["members"] = (len(got) == len(entries) and set(got) == set(want) and
                    all(got[n][1] == w[0] for n, w in want.items()))
    r["modes"] = bool(inner) and all(
        e[1] in ("f", "d") and e[2] == (0o755 if e[0] == top else mode_for(e[0][len(top) + 1:], e[1]))
        for e in under)
    r["excludes"] = junk_ok and bool(inner) and not any(excluded(e[0][len(top) + 1:]) for e in inner)
    r["contents"] = all(n in got and got[n][1] == "f" and got[n][6] == w[2].encode("utf-8")
                        for n, w in want.items() if w[0] == "f")
    r["order"] = len(entries) >= 3 and r["top_dir"] and all(a < b for a, b in zip(names, names[1:]))
    r["owner"] = all(e[3] == 0 and e[4] == 0 for e in entries)
    r["mtime"] = SPEC["mtime"] is not None and all(abs(e[5] - SPEC["mtime"]) < 1 for e in entries)
    for k, v in sorted(r.items()):
        if not v:
            say("%s: rule %s fails" % (top, k))
    return r


def rules():
    return [n for n in NAMES if n != "dist_present"]


def dist_present():
    entries = inspect(ws_path("dist/%s.tar.gz" % SPEC["top_vis"]))
    if entries is None:
        return False
    r = judge(entries, SPEC["want_vis"], SPEC["top_vis"])
    return all(r.get(n) for n in rules())


def no_link_below(root, rel):
    """True when no existing component of root/rel's parent directories is a symlink."""
    cur = root
    for part in rel.split("/")[:-1]:
        cur = os.path.join(cur, part)
        if os.path.islink(cur) or (os.path.exists(cur) and not os.path.isdir(cur)):
            return False
    return True


def hidden_run():
    ws = copy_ws("hid")
    dist = os.path.join(ws, "dist")
    if os.path.islink(dist) or os.path.isfile(dist):
        os.remove(dist)
    else:
        shutil.rmtree(dist, ignore_errors=True)
    for rel, text, mode in SPEC["extras"]:
        if not no_link_below(ws, rel):
            say("a link or file blocks %s" % rel)
            return {}
        p = os.path.join(ws, rel)
        if os.path.islink(p):
            os.remove(p)
        write_file(p, text, mode)
        os.chmod(p, mode)
    junk = [j for j in SPEC["junk"] if os.path.lexists(os.path.join(ws, j))]
    if read_bytes(os.path.join(ws, "package.sh")) is None:
        say("package.sh missing or not a regular file")
        return {}
    code, _out, _err = run(["/bin/sh", "package.sh"], ws, timeout=15, env=base_env())
    if code is None:
        say("package.sh timed out")
    junk_ok = all(os.path.lexists(os.path.join(ws, j)) for j in junk)
    if not junk_ok:
        say("package.sh deleted source files")
    entries = inspect(os.path.join(ws, "dist", "%s.tar.gz" % SPEC["top_hid"]))
    if entries is None:
        return {}
    return judge(entries, SPEC["want_hid"], SPEC["top_hid"], junk_ok)


def main():
    run_checks([("dist_present", dist_present), (rules(), hidden_run)])
