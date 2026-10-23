#!/usr/bin/env python3
"""s1lint: the pack lint for sealed pack s1 (docs/m7-design.md §3.1.1, §8 M7-P07, M7-P08).

    python3 root/sealed/tools/s1lint.py [--release] [--upstream REPO=DIR ...] [--write-index] [--json FILE]

Checks (stdlib only, no generator runs):
  pack      pack.json shape; composition totals (G4 32, reserve 20, 4 canaries); runner files present
  templates every <FAMILY>/<id>/template.json valid (the validator's schema); `_`/`.` dirs are helpers
  P07       licences: each pack licence is permissive, its file's sha256 matches pack.json, upstream commits
            are 40 hex; every non-INIT template has NOTICE and a LICENSE byte-identical to the pack's file;
            E and C templates use their source's licence and pin the same commit pack.json pins.
            --upstream exercism/rust=DIR compares LICENSES/ byte-for-byte with DIR/<path> (and DIR's git HEAD
            with the pinned commit)
  P08       disjointness: E exercise slugs unique, inside pack.json's e_slug_range, and disjoint from the
            visible pool (slugs found in pool_dirs) and the benchmark list; C samples unique.
            No pack file (>= 64 bytes; runner/ and LICENSES excepted) has a byte-identical copy under seed/
            or fixtures/, and no template directory (template.json + gen/run) sits there
  hygiene   no __pycache__, *.pyc or .DS_Store in the pack; gen/run and runner/check executable
  index     pack.json templates[] equals the scanned templates (pending in dev; --write-index rewrites it)
Composition counts and unpinned commits are `pending` in dev and failures with --release.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys

TOOLS = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(os.path.dirname(TOOLS)))
PACK = os.path.join(REPO, "root", "sealed", "packs", "s1")
sys.dont_write_bytecode = True
sys.path.insert(0, TOOLS)
import s1validate  # noqa: E402

HEX40 = re.compile(r"^[0-9a-f]{40}$")
MIN_BYTES = 64
MAX_HASH_BYTES = 16 << 20
SKIP_DIRS = {"target", ".git", "node_modules", "__pycache__", ".build-slots"}
COPY_EXEMPT_TOP = {"runner", "LICENSES"}  # public by design: the shared runner and verbatim licence texts


class Lint:
    def __init__(self, release: bool):
        self.release = release
        self.fails, self.pendings, self.oks = [], [], []

    def fail(self, area: str, msg: str) -> None:
        self.fails.append("%s: %s" % (area, msg))

    def pending(self, area: str, msg: str) -> None:
        (self.fails if self.release else self.pendings).append("%s: %s" % (area, msg))

    def ok(self, area: str) -> None:
        self.oks.append(area)


def sha256_file(p: str) -> str:
    h = hashlib.sha256()
    with open(p, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def load_json(p: str):
    with open(p, "rb") as f:
        return json.loads(f.read().decode("utf-8"))


def walk_files(root: str):
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames[:] = sorted(d for d in dirnames if d not in SKIP_DIRS)
        for n in sorted(filenames):
            yield os.path.join(dirpath, n)


def scan_templates(pack: str, tracked: bool = False) -> list:
    dirs = s1validate.template_dirs(pack)
    return s1validate.only_tracked(dirs) if tracked else dirs


# --------------------------------------------------------------------------------------------- pack


def lint_pack(L: Lint, pack: str) -> dict | None:
    try:
        pj = load_json(os.path.join(pack, "pack.json"))
    except (OSError, ValueError) as e:
        L.fail("pack", "pack.json unreadable: %s" % e)
        return None
    need = {"pack", "version", "format", "created", "families", "composition", "templates", "licences",
            "permissive_spdx", "runner", "disjointness", "no_copies_under"}
    if set(pj) != need:
        L.fail("pack", "pack.json keys must be exactly %s" % sorted(need))
        return None
    if pj["pack"] != "s1" or pj["families"] != ["E", "C", "T", "canary"]:
        L.fail("pack", "pack s1 with families E, C, T, canary")
    comp = pj["composition"]
    fams = ("E", "C", "T", "canary")
    try:
        g4 = sum(comp[f]["g4"] for f in fams)
        res = sum(comp[f]["reserve"] for f in fams if f != "canary")
        if (g4 - comp["canary"]["g4"], res, comp["canary"]["g4"]) != (comp["total"]["g4"], comp["total"]["reserve"],
                                                                    comp["total"]["canary"]):
            L.fail("pack", "composition totals do not add up")
        for f in ("E", "C", "T"):
            if comp[f]["g4"] + comp[f]["reserve"] > comp[f]["authored"]:
                L.fail("pack", "%s: g4 + reserve exceed the authored templates" % f)
        if (comp["total"]["g4"], comp["total"]["reserve"], comp["total"]["canary"]) != (32, 20, 4):
            L.fail("pack", "PROOF-LEAN composition is G4 32, reserve 20, 4 canaries")
    except (KeyError, TypeError) as e:
        L.fail("pack", "composition malformed: %s" % e)
    for rel in pj["runner"]["files"]:
        if not os.path.isfile(os.path.join(pack, rel)):
            L.fail("pack", "runner file %s missing" % rel)
    if not os.access(os.path.join(pack, "runner", "check"), os.X_OK):
        L.fail("hygiene", "runner/check is not executable")
    if not L.fails:
        L.ok("pack")
    return pj


def lint_templates(L: Lint, pack: str, pj: dict, tracked: bool = False) -> list:
    out = []
    for tdir in scan_templates(pack, tracked):
        fam = os.path.basename(os.path.dirname(tdir))
        try:
            t = s1validate.check_template_json(tdir, fam)
        except (s1validate.Fail, ValueError, OSError) as e:
            L.fail("templates", "%s: %s" % (os.path.relpath(tdir, pack), e))
            continue
        out.append((tdir, t))
    ids = [t["id"] for _, t in out]
    dup = sorted({i for i in ids if ids.count(i) > 1})
    if dup:
        L.fail("templates", "duplicate ids %s" % dup)
    comp = pj["composition"]
    for fam in ("E", "C", "T", "canary"):
        have = [t for _, t in out if t["family"] == fam]
        if len(have) != comp[fam]["authored"]:
            L.pending("composition", "%s has %d of %d templates" % (fam, len(have), comp[fam]["authored"]))
        for lang, n in comp[fam].get("langs", {}).items():
            k = sum(1 for t in have if t["lang"] == lang)
            if k != n:
                L.pending("composition", "%s/%s has %d of %d templates" % (fam, lang, k, n))
    kinds = sorted(t["canary"] for _, t in out if t["family"] == "canary")
    if kinds != sorted(comp["canary"]["kinds"]):
        L.pending("composition", "canaries %s, want %s" % (kinds, sorted(comp["canary"]["kinds"])))
    L.ok("templates (%d)" % len(out))
    return out


# ------------------------------------------------------------------------------------------ licences


def lint_licences(L: Lint, pack: str, pj: dict, templates: list, upstream: dict) -> None:
    n0 = len(L.fails)
    lic = pj["licences"]
    permissive = set(pj["permissive_spdx"])
    repo_commit = {}
    for key, e in lic.items():
        if e.get("spdx") not in permissive:
            L.fail("P07", "licence %s: %s is not on the permissive list" % (key, e.get("spdx")))
        if key == "INIT":
            continue
        p = os.path.join(pack, e.get("file") or "")
        if not e.get("file") or not os.path.isfile(p):
            L.fail("P07", "licence %s: file missing" % key)
            continue
        if sha256_file(p) != e.get("sha256"):
            L.fail("P07", "licence %s: %s sha256 differs from pack.json" % (key, e["file"]))
        text = open(p, "rb").read().decode("utf-8", "replace")
        if e["spdx"] == "MIT" and "Permission is hereby granted, free of charge" not in text:
            L.fail("P07", "licence %s: not an MIT text" % key)
        if not e.get("upstream"):
            L.pending("P07", "licence %s: no pinned upstream" % key)
        for repo, u in (e.get("upstream") or {}).items():
            if not isinstance(u, dict) or not HEX40.match(str(u.get("commit"))):
                L.pending("P07", "licence %s: %s commit not pinned" % (key, repo))
            else:
                repo_commit[repo] = (key, u["commit"])
            if repo in upstream:
                d = upstream[repo]
                up = os.path.join(d, u.get("path") or "LICENSE")
                if not os.path.isfile(up):
                    L.fail("P07", "--upstream %s: %s missing" % (repo, up))
                elif open(up, "rb").read() != open(p, "rb").read():
                    L.fail("P07", "licence %s is not byte-identical to %s" % (key, up))
                head = git_head(d)
                if head is not None and head != u.get("commit"):
                    L.fail("P07", "--upstream %s is at %s, pack.json pins %s" % (repo, head[:12], u.get("commit")))
    for tdir, t in templates:
        rel = os.path.relpath(tdir, pack)
        key = t["licence"]
        if key not in lic:
            L.fail("P07", "%s: licence %s is not in pack.json" % (rel, key))
            continue
        if t["family"] in ("E", "C") and key == "INIT":
            L.fail("P07", "%s: E and C templates derive from their source and carry its licence" % rel)
        if key != "INIT":
            for f in ("NOTICE", "LICENSE"):
                if not os.path.isfile(os.path.join(tdir, f)):
                    L.fail("P07", "%s: %s missing" % (rel, f))
            lp = os.path.join(tdir, "LICENSE")
            if os.path.isfile(lp) and sha256_file(lp) != lic[key].get("sha256"):
                L.fail("P07", "%s: LICENSE is not byte-identical to %s" % (rel, lic[key].get("file")))
            src = t.get("source") or {}
            notice = os.path.join(tdir, "NOTICE")
            if src and os.path.isfile(notice) and src.get("repo", "") not in open(notice, encoding="utf-8").read():
                L.fail("P07", "%s: NOTICE does not name %s" % (rel, src.get("repo")))
            if src:
                pinned = repo_commit.get(src.get("repo"))
                if pinned is None or pinned[0] != key:
                    L.fail("P07", "%s: %s is not an upstream of licence %s" % (rel, src.get("repo"), key))
                elif src.get("commit") is None:
                    L.pending("P07", "%s: source.commit not pinned" % rel)
                elif src.get("commit") != pinned[1]:
                    L.fail("P07", "%s: source.commit %s differs from the pinned %s" % (rel, src["commit"][:12],
                                                                                    pinned[1][:12]))
    if len(L.fails) == n0:
        L.ok("P07 licences")


def git_head(d: str) -> str | None:
    if not os.path.exists(os.path.join(d, ".git")):
        return None
    try:
        p = subprocess.run(["git", "-C", d, "rev-parse", "HEAD"], stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                           timeout=30)
        return p.stdout.decode().strip() if p.returncode == 0 else None
    except (OSError, subprocess.TimeoutExpired):
        return None


# ------------------------------------------------------------------------------------- disjointness


def tokens(s: str) -> list:
    return [x for x in re.split(r"[-_./\s]+", s.lower()) if x]


def contains_seq(hay: list, needle: list) -> bool:
    n = len(needle)
    return n > 0 and any(hay[i:i + n] == needle for i in range(len(hay) - n + 1))


def pool_slugs(repo: str, pool_dirs: list) -> tuple:
    """(explicit slugs, path token lists) found in the visible pool: JSON `exercise` values, lines of any
    exercism.txt, and every path component (matched as a token sequence)."""
    explicit, paths = set(), []
    for rel in pool_dirs:
        root = os.path.join(repo, rel)
        if not os.path.isdir(root):
            continue
        for p in walk_files(root):
            paths.append(tokens(os.path.relpath(p, root)))
            name = os.path.basename(p)
            if name == "exercism.txt":
                for line in open(p, encoding="utf-8", errors="replace"):
                    line = line.strip()
                    if line and not line.startswith("#"):
                        explicit.add(line.split("/")[-1])
            elif name.endswith(".json") and os.path.getsize(p) < (4 << 20):
                try:
                    explicit |= set(json_values(load_json(p), "exercise"))
                except (ValueError, OSError):
                    pass
    return explicit, paths


def json_values(obj, key: str):
    if isinstance(obj, dict):
        for k, v in obj.items():
            if k == key and isinstance(v, str):
                yield v
            else:
                yield from json_values(v, key)
    elif isinstance(obj, list):
        for v in obj:
            yield from json_values(v, key)


def lint_disjoint(L: Lint, pack: str, repo: str, pj: dict, templates: list) -> None:
    n0 = len(L.fails)
    dj = pj["disjointness"]
    e = [(t["id"], t["source"]["exercise"]) for _, t in templates if t["family"] == "E"]
    slugs = [s for _, s in e]
    dup = sorted({s for s in slugs if slugs.count(s) > 1})
    if dup:
        L.fail("P08", "E exercise slugs used twice: %s" % dup)
    lo, hi = dj["e_slug_range"].split("-")
    for tid, s in e:
        if not lo <= s[:1] <= hi:
            L.fail("P08", "%s: slug %s is outside the E range %s (the rest belongs to the pool)" % (tid, s,
                                                                                               dj["e_slug_range"]))
    explicit, paths = pool_slugs(repo, dj["pool_dirs"])
    for tid, s in e:
        if s in explicit or any(contains_seq(p, tokens(s)) for p in paths):
            L.fail("P08", "%s: exercise %s also appears in the visible pool" % (tid, s))
    if dj.get("bench_list"):
        bp = os.path.join(repo, dj["bench_list"])
        if os.path.isfile(bp):
            bench = {ln.strip().split("/")[-1] for ln in open(bp, encoding="utf-8") if ln.strip()}
            for tid, s in e:
                if s in bench:
                    L.fail("P08", "%s: exercise %s is in the benchmark list" % (tid, s))
            for s in sorted(bench & explicit):
                L.fail("P08", "exercise %s is in both the pool and the benchmark list" % s)
        else:
            L.pending("P08", "benchmark list %s not present" % dj["bench_list"])
    samples = [t["source"]["sample"] for _, t in templates if t["family"] == "C"]
    dup = sorted({s for s in samples if samples.count(s) > 1})
    if dup:
        L.fail("P08", "C samples used twice: %s" % dup)
    if len(L.fails) == n0:
        L.ok("P08 disjoint sets")


def lint_copies(L: Lint, pack: str, repo: str, pj: dict) -> None:
    n0 = len(L.fails)
    pack_hashes = {}
    for p in walk_files(pack):
        rel = os.path.relpath(p, pack)
        if rel.split(os.sep)[0] in COPY_EXEMPT_TOP or rel == "pack.json":
            continue
        size = os.path.getsize(p)
        if MIN_BYTES <= size <= MAX_HASH_BYTES:
            pack_hashes.setdefault(sha256_file(p), []).append(rel)
    licence_hashes = {e.get("sha256") for e in pj["licences"].values()}
    for top in pj["no_copies_under"]:
        root = os.path.join(repo, top)
        if not os.path.isdir(root):
            continue
        for p in walk_files(root):
            name = os.path.basename(p)
            if name == "template.json" and os.path.isfile(os.path.join(os.path.dirname(p), "gen", "run")):
                L.fail("P08", "a pack template directory under %s: %s" % (top, os.path.relpath(os.path.dirname(p), repo)))
            try:
                size = os.path.getsize(p)
            except OSError:
                continue
            if not MIN_BYTES <= size <= MAX_HASH_BYTES:
                continue
            h = sha256_file(p)
            if h in pack_hashes and h not in licence_hashes:
                L.fail("P08", "%s is a copy of pack file %s" % (os.path.relpath(p, repo), pack_hashes[h][0]))
    if len(L.fails) == n0:
        L.ok("P08 no pack copies under %s" % ", ".join(pj["no_copies_under"]))


def tracked_files(pack: str) -> list:
    p = subprocess.run(["git", "-C", pack, "ls-files", "-z", "--", "."], stdout=subprocess.PIPE,
                       stderr=subprocess.DEVNULL, timeout=60)
    return [x for x in p.stdout.decode().split("\0") if x]


def lint_hygiene(L: Lint, pack: str, templates: list, tracked: bool = False) -> None:
    """No bytecode or Finder litter in the pack (with --tracked: among the committed files only)."""
    n0 = len(L.fails)
    if tracked:
        names = tracked_files(pack)
    else:
        names = [os.path.relpath(os.path.join(dp, n), pack) for dp, dns, fns in os.walk(pack) for n in dns + fns]
    for rel in names:
        parts = rel.split(os.sep) if os.sep in rel else rel.split("/")
        if any(x in ("__pycache__", ".DS_Store") or x.endswith((".pyc", ".pyo")) for x in parts):
            L.fail("hygiene", "%s in the pack" % rel)
    for tdir, _ in templates:
        if not os.access(os.path.join(tdir, "gen", "run"), os.X_OK):
            L.fail("hygiene", "%s/gen/run not executable" % os.path.relpath(tdir, pack))
    if len(L.fails) == n0:
        L.ok("hygiene")


def index_of(pack: str, templates: list) -> list:
    return [{"id": t["id"], "family": t["family"], "lang": t["lang"], "dir": os.path.relpath(tdir, pack)}
            for tdir, t in sorted(templates, key=lambda x: os.path.relpath(x[0], pack))]


def lint_index(L: Lint, pack: str, pj: dict, templates: list) -> None:
    if pj["templates"] != index_of(pack, templates):
        L.pending("index", "pack.json templates[] differs from the scan (run --write-index at release)")
    else:
        L.ok("index")


def run(pack: str = PACK, repo: str = REPO, release: bool = False, upstream: dict | None = None,
        tracked: bool = False) -> Lint:
    L = Lint(release)
    pj = lint_pack(L, pack)
    if pj is None:
        return L
    templates = lint_templates(L, pack, pj, tracked)
    lint_licences(L, pack, pj, templates, upstream or {})
    lint_disjoint(L, pack, repo, pj, templates)
    lint_copies(L, pack, repo, pj)
    lint_hygiene(L, pack, templates, tracked)
    lint_index(L, pack, pj, templates)
    return L


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--pack", default=PACK)
    ap.add_argument("--repo", default=REPO)
    ap.add_argument("--release", action="store_true", help="pending items (counts, pins, index) fail")
    ap.add_argument("--upstream", action="append", default=[], metavar="REPO=DIR",
                    help="a checkout of a licence's upstream repo at its pinned commit, e.g. exercism/rust=DIR")
    ap.add_argument("--write-index", action="store_true", help="rewrite pack.json templates[] from the scan")
    ap.add_argument("--json", help="write the report here")
    ap.add_argument("--tracked", action="store_true",
                    help="only templates whose template.json is committed (CI: lanes' work in progress is ignored)")
    a = ap.parse_args(argv)
    upstream = {}
    for u in a.upstream:
        k, _, v = u.partition("=")
        upstream[k] = v
    if a.write_index:
        pj_path = os.path.join(a.pack, "pack.json")
        pj = load_json(pj_path)
        L0 = Lint(False)
        pj["templates"] = index_of(a.pack, lint_templates(L0, a.pack, pj))
        with open(pj_path, "w") as f:
            f.write(json.dumps(pj, indent=1, ensure_ascii=True) + "\n")
    L = run(a.pack, a.repo, a.release, upstream, a.tracked)
    for o in L.oks:
        print("ok       %s" % o)
    for p in L.pendings:
        print("pending  %s" % p)
    for f in L.fails:
        print("FAIL     %s" % f)
    if a.json:
        with open(a.json, "w") as f:
            json.dump({"ok": L.oks, "pending": L.pendings, "fail": L.fails}, f, indent=1)
    print("s1lint: %d failed, %d pending%s" % (len(L.fails), len(L.pendings), " (release)" if a.release else ""))
    return 1 if L.fails else 0


if __name__ == "__main__":
    sys.exit(main())
