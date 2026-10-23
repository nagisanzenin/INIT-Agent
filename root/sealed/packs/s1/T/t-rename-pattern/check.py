# t-rename-pattern: compare the workspace tree (library/, inbox/) and renames.tsv with the expected plan.
import re as _re


def listing():
    if "t" not in _LS:
        _LS["t"] = tree_listing(WS)
    return _LS["t"]


_LS = {}


def files_under(prefix):
    return {p: v for p, v in listing().items() if p.startswith(prefix + "/") and v[0] == "f"}


def c_layout():
    got = set(files_under("library"))
    want = set(SPEC["library"])
    if got != want:
        say("layout: missing %s extra %s" % (sorted(want - got)[:3], sorted(got - want)[:3]))
    links = [p for p, v in listing().items() if p.startswith("library/") and v[0] in ("l", "o")]
    return got == want and not links


def c_contents():
    lib = files_under("library")
    ok = bool(SPEC["library"]) and all(p in lib and lib[p][1] == h for p, h in SPEC["library"].items())
    return ok


def c_collisions():
    lib = files_under("library")
    if not lib:
        return False
    for p, h in SPEC["collisions"].items():
        if p not in lib or lib[p][1] != h:
            say("collisions: %s wrong or missing" % p)
            return False
    suffixed = {p for p in lib if _re.search(r"-[0-9]+\.[a-z0-9]+$", p)}
    want_suffixed = {p for p in SPEC["collisions"] if _re.search(r"-[0-9]+\.[a-z0-9]+$", p)}
    return suffixed == want_suffixed


def c_leftovers():
    got = files_under("inbox")
    want = SPEC["left"]
    if set(got) != set(want) or any(got[p][1] != h for p, h in want.items()):
        say("leftovers: inbox differs (extra %s, missing %s)" % (sorted(set(got) - set(want))[:3],
                                                                 sorted(set(want) - set(got))[:3]))
        return False
    if SPEC["subdirs"]:
        dirs = sorted(p for p, v in listing().items() if p.startswith("inbox/") and v[0] == "d")
        if dirs != SPEC["left_dirs"]:
            say("leftovers: inbox dirs %s, want %s" % (dirs, SPEC["left_dirs"]))
            return False
    return True


def c_log():
    got = read_text(ws_path("renames.tsv"))
    return got is not None and norm_lines(got) == norm_lines(SPEC["log"])


def main():
    run_checks([
        ("layout", c_layout),
        ("contents", c_contents),
        ("collisions", c_collisions),
        ("leftovers", c_leftovers),
        ("log", c_log),
    ])
