# t-checksum-manifest: compare release/MANIFEST.sha256 and release/VERIFY.txt with the expected ones.
import re as _re

LINE = _re.compile(r"^([0-9a-f]{64})  (.+)$")


def manifest():
    t = read_text(ws_path("release/MANIFEST.sha256"))
    if t is None:
        return None
    out = {}
    for ln in norm_lines(t):
        m = LINE.match(ln)
        if not m:
            return None
        out[m.group(2)] = m.group(1)
    return out


def c_paths():
    m = manifest()
    if m is None:
        return False
    ok = set(m) == set(SPEC["paths"])
    if not ok:
        say("manifest paths: extra %s missing %s" % (sorted(set(m) - set(SPEC["paths"]))[:3],
                                                    sorted(set(SPEC["paths"]) - set(m))[:3]))
    return ok


def c_hashes():
    m = manifest()
    return m is not None and all(m.get(p) == h for p, h in SPEC["hashes"].items())


def c_exact():
    return norm_lines(read_text(ws_path("release/MANIFEST.sha256"))) == norm_lines(SPEC["manifest"])


def verify_lines():
    return norm_lines(read_text(ws_path("release/VERIFY.txt")))


def c_verify():
    got = verify_lines()
    want = norm_lines(SPEC["verify"])
    return got is not None and len(got) >= 1 and got[:-1] == want[:-1]


def c_summary():
    got = verify_lines()
    return bool(got) and got[-1] == norm_lines(SPEC["verify"])[-1]


def main():
    run_checks([
        ("manifest_paths", c_paths),
        ("manifest_hashes", c_hashes),
        ("manifest_exact", c_exact),
        ("verify", c_verify),
        ("verify_summary", c_summary),
    ])
