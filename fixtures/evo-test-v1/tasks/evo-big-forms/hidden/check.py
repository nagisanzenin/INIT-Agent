"""evo-big-forms check: is widgets.py migrated as MIGRATION.md says?

Signatures are read statically from widgets.py (ast). Behaviour is observed by hidden/harness.py, which runs in
a subprocess, imports the workspace's widgets.py and records the renderer calls of probe calls (inputs only).
Expectations come from the generator's model (hidden/fgen.py). One item per (rule, section), plus one per
section for tokens that must not change, plus one per exception kind. Prints PASS/FAIL <item> lines."""
import ast
import json
import os
import subprocess
import sys

HID = os.path.join(os.environ["INIT_CHECK_DIR"], "hidden")
sys.path.insert(0, HID)
import fgen as G  # noqa: E402

WS = os.getcwd()
SCRATCH = os.environ.get("S1_SCRATCH") or "/tmp"
ASPECTS = ["attrs", "bools", "defaults", "names", "deprecated", "theme", "unchanged"]
EXC_ITEMS = ["exceptions/legacy-keys", "exceptions/bare-bools"]
BAD = ["Bad Name", "", "x" * 41, 7, None, "1abc", "a-b", "ok_name ", "Name", True]
ODD = ["a", "z9", "a_b_c", "q" * 40]


def names():
    return ["%s/%s" % (a, s) for a in ASPECTS for s in G.SECTION_NAMES] + EXC_ITEMS


def _lit(node):
    try:
        v = ast.literal_eval(node)
    except Exception:  # noqa: BLE001
        return ("expr", ast.dump(node))
    return (type(v).__name__, repr(v))


def static_sigs():
    try:
        with open(os.path.join(WS, "widgets.py"), "rb") as f:
            tree = ast.parse(f.read().decode("utf-8"))
    except Exception:  # noqa: BLE001
        return {}
    out = {}
    for n in tree.body:
        if isinstance(n, ast.FunctionDef):
            a = n.args
            pos = list(a.args)
            defs = [None] * (len(pos) - len(a.defaults)) + list(a.defaults)
            sig = [(x.arg, False, d is not None, _lit(d) if d is not None else None) for x, d in zip(pos, defs)]
            sig += [(x.arg, True, d is not None, _lit(d) if d is not None else None)
                    for x, d in zip(a.kwonlyargs, a.kw_defaults)]
            if a.vararg or a.kwarg:
                sig.append(("<var>", None, None, None))
            out[n.name] = sig
    return out


def norm(sig):
    return [(n, kw, hd, (type(d).__name__, repr(d)) if hd else None) for n, kw, hd, d in sig]


def plan(cat):
    probes, info = [], {}
    byn = G.by_name(cat)

    def add(pid, fn, kw):
        probes.append({"id": pid, "fn": fn, "kwargs": kw})
        return (pid, kw)
    for idx, w in enumerate(cat["widgets"]):
        if w["dep"]:
            continue
        salt = 41 * idx + 7
        i = {"full": add(w["name"] + ":full", w["name"], G.kwargs(w, salt, "h", "full")),
             "defaults": add(w["name"] + ":def", w["name"], G.kwargs(w, salt + 1, "h", "defaults")),
             "min": add(w["name"] + ":min", w["name"], G.kwargs(w, salt + 2, "h", "min"))}
        kw = G.kwargs(w, salt + 3, "h", "full")
        kw["theme"] = "t%d" % idx
        i["theme"] = add(w["name"] + ":theme", w["name"], kw)
        bad = []
        for j, p in enumerate([p for p in w["params"] if p["role"] == "name"]):
            for k in range(2):
                kw = G.kwargs(w, salt + 4, "h", "min")
                kw[p["name"]] = BAD[(idx + j + 5 * k) % len(BAD)]
                bad.append(add("%s:bad%d.%d" % (w["name"], j, k), w["name"], kw))
        kw = G.kwargs(w, salt + 5, "h", "full")
        kw["name"] = ODD[idx % len(ODD)]
        i["odd"] = add(w["name"] + ":odd", w["name"], kw)
        i["bad"] = bad
        info[w["name"]] = i
    for w in cat["widgets"]:
        if w["dep"]:
            r = info[w["dep"]]
            kw = dict(r["theme"][1])
            info[w["name"]] = {"full": add(w["name"] + ":full", w["name"], kw), "rep": r["theme"],
                               "bad": add(w["name"] + ":bad", w["name"], r["bad"][0][1]), "repbad": r["bad"][0]}
    del byn
    return probes, info


def run_harness(probes):
    pin = os.path.join(SCRATCH, "evo-forms-probes.json")
    pout = os.path.join(SCRATCH, "evo-forms-observed.json")
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
    return dict((p["id"], p) for p in out["probes"])


def key_of(tok):
    return tok.split("=", 1)[0]


class Judge:
    def __init__(self, cat, obs, sigs, info):
        self.cat, self.obs, self.sigs, self.info = cat, obs, sigs, info

    def toks(self, pid):
        o = self.obs.get(pid)
        if not o or o["exc"] or len(o["calls"]) != 1 or not o["ret"]:
            return None
        return o["calls"][0]

    def exp(self, w, kw):
        return G.v2_tokens(w, self.cat, kw)

    def keyset(self, p, w):
        return {p["key"], G.key2(w, p, self.cat), G.RENAME.get(p["key"], p["key"])}

    def part(self, toks, keys):
        return sorted(t for t in toks[1:] if key_of(t) in keys)

    def opts(self, w, pred):
        return [p for p in w["params"] if p["role"] == "opt" and pred(p)]

    def attrs(self, w):
        t = self.toks(self.info[w["name"]]["full"][0])
        if t is None:
            return False
        e = self.exp(w, self.info[w["name"]]["full"][1])[1]
        for p in self.opts(w, lambda p: p["key"] in G.RENAME):
            ks = self.keyset(p, w)
            got = [key_of(x) for x in t[1:] if key_of(x) in ks]
            want = [key_of(x) for x in e[1:] if key_of(x) in ks]
            if got != want:
                return False
            if p["type"] != "bool" and self.part(t, ks) != self.part(e, ks):
                return False
        return True

    def bools(self, w):
        for k in ("full", "defaults", "min"):
            pid, kw = self.info[w["name"]][k]
            t = self.toks(pid)
            if t is None:
                return False
            e = self.exp(w, kw)[1]
            for p in self.opts(w, lambda p: p["type"] == "bool"):
                ks = self.keyset(p, w)
                norm_t = sorted(G.key2(w, p, self.cat) + x[len(key_of(x)):] for x in t[1:] if key_of(x) in ks)
                if norm_t != self.part(e, ks):
                    return False
        return True

    def defaults(self, w):
        for k in ("defaults", "min"):
            pid, kw = self.info[w["name"]][k]
            t = self.toks(pid)
            if t is None:
                return False
            e = self.exp(w, kw)[1]
            for p in self.opts(w, lambda p: p["type"] != "bool" and p["default"] is not None):
                ks = self.keyset(p, w)
                got = sorted(x[len(key_of(x)):] for x in t[1:] if key_of(x) in ks)
                want = sorted(x[len(key_of(x)):] for x in e[1:] if key_of(x) in ks)
                if got != want:
                    return False
        return True

    def names_(self, w):
        for pid, kw in self.info[w["name"]]["bad"]:
            o = self.obs.get(pid)
            e = self.exp(w, kw)
            if not o or o["exc"] != ["ValueError", e[1]] or o["calls"]:
                return False
        t = self.toks(self.info[w["name"]]["odd"][0])
        return t is not None

    def deprecated(self, d):
        rep = G.by_name(self.cat)[d["dep"]]
        if self.sigs.get(d["name"]) != norm(G.v2_sig(rep)):
            return False
        i = self.info[d["name"]]
        msg = [["DeprecationWarning", G.dep_message(d)]]
        a, r = self.obs.get(i["full"][0]), self.obs.get(i["rep"][0])
        if not a or not r or a["exc"] or r["exc"] or a["warns"] != msg or not a["ret"] or a["calls"] != r["calls"]:
            return False
        b, rb = self.obs.get(i["bad"][0]), self.obs.get(i["repbad"][0])
        return bool(b and rb) and b["warns"] == msg and b["exc"] == rb["exc"] and b["calls"] == rb["calls"]

    def theme(self, w):
        sig = self.sigs.get(w["name"])
        if not sig or norm([G.v2_sig(w)[-1]]) != sig[-1:]:
            return False
        t = self.toks(self.info[w["name"]]["theme"][0])
        t0 = self.toks(self.info[w["name"]]["full"][0])
        if t is None or t0 is None:
            return False
        want = "theme=%s" % self.info[w["name"]]["theme"][1]["theme"]
        return [x for x in t[1:] if key_of(x) == "theme"] == [want] and not [x for x in t0[1:] if key_of(x) == "theme"]

    def unchanged(self, w):
        sig = self.sigs.get(w["name"])
        if not sig or sig[:len(w["params"])] != norm(G.v1_sig(w)):
            return False
        for k in ("full", "min"):
            pid, kw = self.info[w["name"]][k]
            t = self.toks(pid)
            if t is None:
                return False
            e = self.exp(w, kw)[1]
            if t[0] != e[0]:
                return False
            keys = set(["name", "group"]) | set(p["key"] for p in self.opts(
                w, lambda p: p["type"] != "bool" and p["key"] not in G.RENAME))
            if k == "full" and self.part(t, keys) != self.part(e, keys):
                return False
        return True

    def legacy(self, w):
        for k in ("full", "defaults"):
            pid, kw = self.info[w["name"]][k]
            t = self.toks(pid)
            e = self.exp(w, kw)[1]
            if t is None or sorted(t[1:]) != sorted(e[1:]) or t[0] != e[0]:
                return False
        return True

    def barebools(self, w):
        return self.legacy(w)


def groups(cat):
    g = {}
    for s in G.SECTION_NAMES:
        ws = [w for w in cat["widgets"] if w["section"] == s and not w["dep"]]
        g["attrs/" + s] = ("attrs", [w for w in ws if w["name"] not in cat["legacy"]
                                     and any(p.get("key") in G.RENAME for p in w["params"])])
        g["bools/" + s] = ("bools", [w for w in ws if w["name"] not in cat["barebools"]
                                     and any(p.get("type") == "bool" for p in w["params"])])
        g["defaults/" + s] = ("defaults", [w for w in ws if any(
            p.get("type") not in (None, "bool") and p.get("default") is not None for p in w["params"])])
        g["names/" + s] = ("names_", ws)
        g["deprecated/" + s] = ("deprecated", [w for w in cat["widgets"] if w["section"] == s and w["dep"]])
        g["theme/" + s] = ("theme", ws)
        g["unchanged/" + s] = ("unchanged", ws)
    byn = G.by_name(cat)
    g["exceptions/legacy-keys"] = ("legacy", [byn[n] for n in cat["legacy"]])
    g["exceptions/bare-bools"] = ("barebools", [byn[n] for n in cat["barebools"]])
    assert all(v[1] for v in g.values()), [k for k, v in g.items() if not v[1]]
    return g


def main():
    cat = G.build()
    probes, info = plan(cat)
    sigs = static_sigs()
    obs = run_harness(probes) if sigs else None
    g = groups(cat)
    j = Judge(cat, obs, sigs, info) if obs else None
    for item in names():
        fn, ws = g[item]
        bad = [w["name"] for w in ws if not getattr(j, fn)(w)] if j else ["<no observations>"]
        if bad and os.environ.get("EVO_CHECK_DEBUG"):
            sys.stderr.write("%s: %s\n" % (item, " ".join(bad)))
        print("%s %s" % ("FAIL" if bad else "PASS", item))


if __name__ == "__main__":
    main()
