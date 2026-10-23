"""evo-big-rules check: is rules.py migrated as MIGRATION.md says?

Signatures are read statically from rules.py (ast). Behaviour is observed by hidden/harness.py, which runs in a
subprocess, imports the workspace's rules.py and calls rule functions on probe records and contexts (inputs
only). Expectations come from the generator's model (hidden/rgen.py). One item per (aspect, section), plus
retirements, merges, the registry and one per exception kind. Prints PASS/FAIL <item> lines."""
import ast
import json
import os
import subprocess
import sys

HID = os.path.join(os.environ["INIT_CHECK_DIR"], "hidden")
sys.path.insert(0, HID)
import rgen as G  # noqa: E402

WS = os.getcwd()
SCRATCH = os.environ.get("S1_SCRATCH") or "/tmp"
ASPECTS = ["shape", "codes", "severity", "context", "composed", "unchanged"]
GLOBAL_ITEMS = ["retired/shims", "retired/merged", "registry", "exceptions/keep-code", "exceptions/severity",
                "exceptions/strict-subrules"]
CTX_TPL = ("not_future", "max_age", "strict_required")
SIG = [("record", False, False), ("ctx", False, False)]


def names():
    return ["%s/%s" % (a, s) for a in ASPECTS for s in G.SECTION_NAMES] + GLOBAL_ITEMS


def static_sigs():
    try:
        with open(os.path.join(WS, "rules.py"), "rb") as f:
            tree = ast.parse(f.read().decode("utf-8"))
    except Exception:  # noqa: BLE001
        return {}
    out = {}
    for n in tree.body:
        if isinstance(n, ast.FunctionDef):
            a = n.args
            pos = list(getattr(a, "posonlyargs", [])) + list(a.args)
            nd = len(a.defaults)
            sig = [(x.arg, False, i >= len(pos) - nd) for i, x in enumerate(pos)]
            sig += [(x.arg, True, d is not None) for x, d in zip(a.kwonlyargs, a.kw_defaults)]
            if a.vararg is not None or a.kwarg is not None:
                sig.append(("<var>", None, None))
            out[n.name] = sig
    return out


def plan(cat):
    """probes [{id, fn, rec, ctx}] and per rule id: list of (probe id, record, ctx index)."""
    byid = G.by_id(cat)
    secs = dict((s[0], s) for s in G.SECTIONS)
    probes, per = [], {}

    def add(rid, fn, recs, ctxs, tag):
        out = per.setdefault((rid, tag), [])
        for i, rec in enumerate(recs):
            for c in ctxs:
                pid = "%s:%s:%d:%d" % (rid, tag, i, c)
                probes.append({"id": pid, "fn": fn, "rec": rec, "ctx": G.ALT_CTX[c]})
                out.append((pid, rec, c))
    for k, r in enumerate(cat["rules"]):
        if r["id"] in cat["retired"] or r["id"] in cat["merged"]:
            continue
        recs = G.probe_records(cat, r, secs[r["section"]], 3 + 11 * k)
        ctxs = [0, 1, 2, 3] if r["tpl"] in CTX_TPL or r["tpl"] == "composed" else [0]
        add(r["id"], r["fn"], recs, ctxs, "own")
    for old, new in sorted(cat["retired"].items()):
        n = byid[new]
        recs = G.probe_records(cat, n, secs[n["section"]], 5)
        add(old, byid[old]["fn"], recs, [0, 1], "shim")
        add(new, n["fn"], recs, [0, 1], "shim")
    for b, a in sorted(cat["merged"].items()):
        ra, rb = byid[a], byid[b]
        recs = G.probe_records(cat, ra, secs[ra["section"]], 9) + G.probe_records(cat, rb, secs[rb["section"]], 13)[1:]
        add(a, ra["fn"], recs, [0, 2], "merge")
        add(b, rb["fn"], recs, [0, 2], "merge")
    return probes, per


def run_harness(probes):
    pin = os.path.join(SCRATCH, "evo-rules-probes.json")
    pout = os.path.join(SCRATCH, "evo-rules-observed.json")
    with open(pin, "w") as f:
        f.write(json.dumps(probes))
    env = {"PATH": "/usr/bin:/bin", "HOME": SCRATCH, "TMPDIR": SCRATCH, "LC_ALL": "C", "LANG": "C",
           "PYTHONHASHSEED": "0", "PYTHONDONTWRITEBYTECODE": "1", "PYTHONPATH": WS}
    try:
        subprocess.run([sys.executable, "-S", "-s", "-B", os.path.join(HID, "harness.py"), pin, pout, WS],
                       cwd=SCRATCH, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, timeout=20)
        with open(pout) as f:
            out = json.load(f)
    except Exception:  # noqa: BLE001 - a crash or a timeout fails every item
        return None
    if out.get("import"):
        return None
    out["by"] = dict((p["id"], p) for p in out["probes"])
    return out


def same(a, b):
    if isinstance(a, (list, tuple)) and isinstance(b, (list, tuple)):
        return len(a) == len(b) and all(same(x, y) for x, y in zip(a, b))
    return type(a) == type(b) and a == b


def finding_ok(obs, exp, fields):
    """obs: the observed probe; exp: the expected result; fields: the indexes that must match."""
    if obs is None or obs["exc"]:
        return False
    res = obs["res"]
    if exp is None:
        return res is None
    if not isinstance(res, list) or len(res) != 4:
        return False
    return all(same(res[i], exp[i]) for i in fields)


class Judge:
    def __init__(self, cat, obs, sigs, per):
        self.cat, self.obs, self.sigs, self.per = cat, obs, sigs, per
        self.byid = G.by_id(cat)

    def o(self, pid):
        return self.obs["by"].get(pid)

    def exp(self, rid, rec, c):
        return G.v2(self.cat, rid, rec, G.ALT_CTX[c], self.byid)

    def sig_ok(self, rid):
        return [e[:3] for e in self.sigs.get(self.byid[rid]["fn"], [])] == SIG

    def shape(self, rid):
        if not self.sig_ok(rid):
            return False
        return all(finding_ok(self.o(pid), self.exp(rid, rec, c), (0, 3))
                   for pid, rec, c in self.per[(rid, "own")] if c == 0)

    def _field(self, rid, idx, want=None):
        strict = self.byid[rid]["tpl"] == "strict_required"
        seen = 0
        for pid, rec, c in self.per[(rid, "own")]:
            if c != (3 if strict else 0):
                continue
            e = self.exp(rid, rec, c)
            if e is None:
                continue
            ob = self.o(pid)
            if ob is None or ob["exc"]:
                return False
            res = ob["res"]
            if res is None and strict and want is None:
                continue
            if not isinstance(res, list) or len(res) != 4 or res[0] != e[0]:
                return False
            if not same(res[idx], want if want is not None else e[idx]):
                return False
            seen += 1
        return seen > 0 or (strict and want is None)

    def codes(self, rid):
        return self._field(rid, 1)

    def severity(self, rid):
        return self._field(rid, 2)

    def context(self, rid):
        return all(finding_ok(self.o(pid), self.exp(rid, rec, c), (0, 3)) for pid, rec, c in self.per[(rid, "own")])

    def composed(self, rid):
        return self.sig_ok(rid) and self.context(rid)

    def unchanged(self, rid):
        for pid, rec, c in self.per[(rid, "own")]:
            ob = self.o(pid)
            if c != 0 or ob is None or ob["exc"]:
                return False
            if (ob["res"] is None) != (self.exp(rid, rec, c) is None):
                return False
        return True

    def shim(self, old):
        new = self.cat["retired"][old]
        if not self.sig_ok(old):
            return False
        for (po, rec, c), (pn, _, _) in zip(self.per[(old, "shim")], self.per[(new, "shim")]):
            a, b = self.o(po), self.o(pn)
            if not a or not b or a["exc"] or b["exc"] or not same(a["res"], b["res"]):
                return False
            if not finding_ok(b, self.exp(new, rec, c), (0,)):
                return False
        return True

    def merge(self, b):
        a = self.cat["merged"][b]
        if not self.sig_ok(a) or not self.sig_ok(b):
            return False
        for (pa, rec, c), (pb, _, _) in zip(self.per[(a, "merge")], self.per[(b, "merge")]):
            oa, ob = self.o(pa), self.o(pb)
            if not finding_ok(oa, self.exp(a, rec, c), (0, 1, 2, 3)):
                return False
            if not ob or ob["exc"] or not same(oa["res"], ob["res"]):
                return False
        return True

    def registry(self, _):
        reg = self.obs.get("rules")
        if not isinstance(reg, dict) or "__error__" in reg:
            return False
        live = G.live_ids(self.cat)
        if sorted(reg) != sorted(live):
            return False
        for rid in live:
            r = self.byid[rid]
            if reg[rid] != [r["fn"], True, r["cat"]]:
                return False
        return True

    def keep_code(self, rid):
        return self._field(rid, 1, want=self.byid[rid]["code"])

    def pinned(self, rid):
        return self._field(rid, 2, want=self.cat["pinned"][rid])

    def strict_sub(self, rid):
        return self.composed(rid)


def groups(cat):
    special = set(cat["retired"]) | set(cat["merged"]) | set(cat["merged"].values())
    g = {}
    for s in G.SECTION_NAMES:
        rs = [r for r in cat["rules"] if r["section"] == s and r["id"] not in special]
        plain = [r["id"] for r in rs if r["tpl"] != "composed"]
        g["shape/" + s] = ("shape", [r["id"] for r in rs if r["id"] not in cat["strict_sub"]])
        g["codes/" + s] = ("codes", [x for x in plain if x not in cat["keep_code"]])
        g["severity/" + s] = ("severity", [x for x in plain if x not in cat["pinned"]])
        g["context/" + s] = ("context", [r["id"] for r in rs if r["tpl"] in CTX_TPL])
        g["composed/" + s] = ("composed", [r["id"] for r in rs if r["tpl"] == "composed"
                                           and r["id"] not in cat["strict_sub"]])
        g["unchanged/" + s] = ("unchanged", [r["id"] for r in rs if r["tpl"] not in CTX_TPL and r["tpl"] != "composed"])
    g["retired/shims"] = ("shim", sorted(cat["retired"]))
    g["retired/merged"] = ("merge", sorted(cat["merged"]))
    g["registry"] = ("registry", ["*"])
    g["exceptions/keep-code"] = ("keep_code", list(cat["keep_code"]))
    g["exceptions/severity"] = ("pinned", sorted(cat["pinned"]))
    g["exceptions/strict-subrules"] = ("strict_sub", list(cat["strict_sub"]))
    assert all(len(v[1]) >= 1 for v in g.values()), [k for k, v in g.items() if not v[1]]
    return g


def main():
    cat = G.build()
    probes, per = plan(cat)
    sigs = static_sigs()
    obs = run_harness(probes) if sigs else None
    g = groups(cat)
    j = Judge(cat, obs, sigs, per) if obs else None
    for item in names():
        fn, ids = g[item]
        bad = [x for x in ids if not getattr(j, fn)(x)] if j else ["<no observations>"]
        if bad and os.environ.get("EVO_CHECK_DEBUG"):
            sys.stderr.write("%s: %s\n" % (item, " ".join(bad)))
        print("%s %s" % ("FAIL" if bad else "PASS", item))


if __name__ == "__main__":
    main()
