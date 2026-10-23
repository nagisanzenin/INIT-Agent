# t-mirror-sync: compare dst/ (files, contents, directories) and sync.log with the expected sync.


def dst_listing():
    if "t" not in _LS:
        _LS["t"] = tree_listing(ws_path("dst")) if os.path.isdir(ws_path("dst")) else {}
    return _LS["t"]


_LS = {}


def dst_files():
    return {p: v[1] for p, v in dst_listing().items() if v[0] == "f"}


def has_all(paths):
    got = dst_files()
    fin = SPEC["final"]
    bad = [p for p in paths if got.get(p) != fin[p]]
    if bad:
        say("wrong or missing in dst: %s" % bad[:4])
    return not bad


def c_copied():
    return has_all(SPEC["copy"])


def c_updated():
    return has_all(SPEC["update"])


def c_deleted():
    got = dst_files()
    if any(p in got for p in SPEC["delete"]):
        return False
    dirs = sorted(p for p, v in dst_listing().items() if v[0] == "d")
    if dirs != SPEC["final_dirs"]:
        say("dst dirs %s, want %s" % (sorted(set(dirs) ^ set(SPEC["final_dirs"]))[:4], "pruned"))
        return False
    others = [p for p, v in dst_listing().items() if v[0] in ("l", "o")]
    return not others


def c_rules():
    got = dst_files()
    if not SPEC["copy"] or not all(p in got for p in SPEC["copy"]):
        return False  # rules only count in a sync that happened
    for p, h in SPEC["ruled"].items():
        if h is None and p in got:
            say("rules: %s should not be in dst" % p)
            return False
        if h is not None and got.get(p) != h:
            say("rules: %s changed or missing" % p)
            return False
    return True


def c_log():
    got = read_text(ws_path("sync.log"))
    return got is not None and norm_lines(got) == norm_lines(SPEC["log"])


def main():
    run_checks([
        ("copied", c_copied),
        ("updated", c_updated),
        ("deleted", c_deleted),
        ("rules", c_rules),
        ("log", c_log),
    ])
