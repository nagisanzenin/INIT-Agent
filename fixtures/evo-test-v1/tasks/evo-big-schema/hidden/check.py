"""evo-big-schema check: is models.py migrated as MIGRATION.md says?

Constructor signatures are read statically from models.py (ast). Behaviour is observed by hidden/harness.py,
which runs in a subprocess, imports the workspace's models.py and builds instances from probe inputs (inputs
only). Expectations come from the generator's model (hidden/sgen.py). One item per (rule, section), plus one
per exception kind. Prints PASS/FAIL <item> lines."""
import ast
import json
import os
import subprocess
import sys

HID = os.path.join(os.environ["INIT_CHECK_DIR"], "hidden")
sys.path.insert(0, HID)
import sgen as G  # noqa: E402

WS = os.getcwd()
SCRATCH = os.environ.get("S1_SCRATCH") or "/tmp"
ASPECTS = ["keys", "read", "compat", "units", "enums", "required", "deprecated"]
EXC_ITEMS = ["exceptions/legacy-wire", "exceptions/imperial", "exceptions/enum-keep"]


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
        with open(os.path.join(WS, "models.py"), "rb") as f:
            tree = ast.parse(f.read().decode("utf-8"))
    except Exception:  # noqa: BLE001
        return {}
    out = {}
    for c in tree.body:
        if not isinstance(c, ast.ClassDef):
            continue
        for n in c.body:
            if isinstance(n, ast.FunctionDef) and n.name == "__init__":
                a = n.args
                pos = list(a.args)
                defs = [None] * (len(pos) - len(a.defaults)) + list(a.defaults)
                sig = [(x.arg, d is not None, _lit(d) if d is not None else None) for x, d in zip(pos[1:], defs[1:])]
                if a.kwonlyargs or a.vararg or a.kwarg:
                    sig.append(("<other>", None, None))
                out[c.name] = sig
    return out


def norm(sig):
    return [(n, hd, (type(d).__name__, repr(d)) if hd else None) for n, hd, d in sig]


def decode(x):
    if isinstance(x, dict) and "__dict__" in x:
        return dict((k, decode(v)) for k, v in x["__dict__"])
    if isinstance(x, list):
        return [decode(v) for v in x]
    return x


def plan(cat):
    byn = G.by_name(cat)
    probes, info = [], {}

    def add(pid, **kw):
        kw["id"] = pid
        probes.append(kw)
        return pid
    for idx, c in enumerate(cat["classes"]):
        name = c["name"]
        salt = 37 * idx + 3
        if c["dep"]:
            rep = byn[c["dep"]]
            kw = G.kwargs_v2(rep, cat, salt, "h")
            attrs = [G.v2_attr(rep, f, cat) for f in rep["fields"]]
            info[name] = {"new": (add(name + ":new", kind="new", cls=name, kwargs=kw, attrs=attrs, isinst=rep["name"]), kw),
                          "rep": (add(name + ":rep", kind="new", cls=rep["name"], kwargs=kw, attrs=attrs), kw)}
            continue
        attrs = [G.v2_attr(c, f, cat) for f in c["fields"]]
        i = {}
        kw = G.kwargs_v2(c, cat, salt, "h")
        i["new"] = (add(name + ":new", kind="new", cls=name, kwargs=kw, attrs=attrs), kw)
        kwm = G.kwargs_v2(c, cat, salt + 1, "h", full=False)
        i["min"] = (add(name + ":min", kind="new", cls=name, kwargs=kwm, attrs=attrs), kwm)
        d2 = G.payload_v2(c, cat, salt + 2, "h")
        i["r2"] = (add(name + ":r2", kind="read", cls=name, data=d2, attrs=attrs), d2)
        d2m = G.payload_v2(c, cat, salt + 3, "h", full=False)
        i["r2m"] = (add(name + ":r2m", kind="read", cls=name, data=d2m, attrs=attrs), d2m)
        d1 = G.payload_v1(c, cat, salt + 4, "h")
        i["r1"] = (add(name + ":r1", kind="read", cls=name, data=d1, attrs=attrs), d1)
        d1m = G.payload_v1(c, cat, salt + 10, "h", full=False)
        i["r1m"] = (add(name + ":r1m", kind="read", cls=name, data=d1m, attrs=attrs), d1m)
        for key, kind, sv1, full in (("u2", "unit", False, True), ("u2m", "unit", False, False),
                                     ("e2", "enum", True, True), ("e2b", "enum", False, True)):
            d = both(c, cat, salt + 11, kind, sv1, full)
            i[key] = (add("%s:%s" % (name, key), kind="read", cls=name, data=d, attrs=attrs), d)
        miss = []
        req = [f for f in c["fields"] if f["req"]]
        for j, f in enumerate(req[:3]):
            d = both(c, cat, salt + 5 + j, None, False, True, drop=[f])
            miss.append((add("%s:miss%d" % (name, j), kind="read", cls=name, data=d, attrs=attrs), d))
        if len(req) >= 2:
            d = both(c, cat, salt + 9, None, False, True, drop=req[1:])
            miss.append((add("%s:missall" % name, kind="read", cls=name, data=d, attrs=attrs), d))
        i["miss"] = miss
        info[name] = i
    return probes, info


def both(c, cat, salt, kind, v1_values, full, drop=()):
    """A payload with every field under its v1 and its v2 wire key, except fields of `kind` (only under the
    v2 key, with v1 or v2 enum values) and the `drop` fields (absent); optional fields only when full."""
    d = {}
    for i, f in enumerate(c["fields"]):
        if f in drop or not (f["req"] or full):
            continue
        if f["kind"] == kind:
            d[G.v2_key(c, f, cat)] = G.sample(f, salt + i, "h", v1=v1_values)
        else:
            v = G.sample(f, salt + i, "h", v1=c["name"] in cat["enumkeep"])
            d[f["v1key"]] = v
            d[G.v2_key(c, f, cat)] = v
    return d


def run_harness(probes):
    pin = os.path.join(SCRATCH, "evo-schema-probes.json")
    pout = os.path.join(SCRATCH, "evo-schema-observed.json")
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
    by = {}
    for p in out["probes"]:
        p["dict"] = decode(p["dict"])
        by[p["id"]] = p
    return by


def same(a, b):
    num = (int, float)
    if isinstance(a, num) and isinstance(b, num) and not isinstance(a, bool) and not isinstance(b, bool) \
            and (isinstance(a, float) or isinstance(b, float)):
        return isinstance(a, float) == isinstance(b, float) and abs(a - b) <= 1e-9 * max(1.0, abs(b))
    if isinstance(a, dict) and isinstance(b, dict):
        return set(a) == set(b) and all(same(a[k], b[k]) for k in a)
    return type(a) == type(b) and a == b


class Judge:
    def __init__(self, cat, obs, sigs, info):
        self.cat, self.obs, self.sigs, self.info = cat, obs, sigs, info
        self.byn = G.by_name(cat)

    def ok(self, pid):
        o = self.obs.get(pid)
        return o if (o and not o["exc"] and isinstance(o["dict"], dict) and "__error__" not in o["dict"]) else None

    def fields(self, c, kinds):
        return [f for f in c["fields"] if f["kind"] in kinds]

    def sig_at(self, c, kinds):
        sig = self.sigs.get(c["name"])
        want = norm(G.v2_sig(c, self.cat))
        if sig is None or len(sig) != len(want):
            return False
        return all(sig[j] == want[j] for j, f in enumerate(c["fields"]) if f["kind"] in kinds)

    def wire_part(self, c, d, fs):
        return dict((G.v2_key(c, f, self.cat), d[G.v2_key(c, f, self.cat)]) for f in fs if G.v2_key(c, f, self.cat) in d)

    def dict_ok(self, c, pid, kw, kinds, strict_keys):
        o = self.ok(pid)
        if not o:
            return False
        exp = G.v2_to_dict(c, self.cat, kw)
        fs = self.fields(c, kinds)
        if not same(self.wire_part(c, o["dict"], fs), self.wire_part(c, exp, fs)):
            return False
        if strict_keys:
            stale = set(f["v1key"] for f in fs) - set(G.v2_key(c, f, self.cat) for f in c["fields"])
            if stale & set(o["dict"]):
                return False
        return True

    def attrs_ok(self, c, pid, data, kinds):
        o = self.ok(pid)
        if not o:
            return False
        exp = G.v2_from_dict(c, self.cat, data)
        if isinstance(exp, tuple):
            return False
        return all(same(o["attrs"].get(G.v2_attr(c, f, self.cat)), exp[G.v2_attr(c, f, self.cat)])
                   for f in self.fields(c, kinds))

    # -- items
    def keys(self, c):
        i = self.info[c["name"]]
        return all(self.dict_ok(c, i[k][0], i[k][1], ("str", "int", "float", "bool"), True) for k in ("new", "min"))

    def read(self, c):
        i = self.info[c["name"]]
        return all(self.attrs_ok(c, i[k][0], i[k][1], ("str", "int", "float", "bool")) for k in ("r2", "r2m"))

    def compat(self, c):
        i = self.info[c["name"]]
        allk = ("str", "int", "float", "bool", "enum", "unit")
        return all(self.attrs_ok(c, i[k][0], i[k][1], allk) for k in ("r1", "r1m"))

    def units(self, c):
        i = self.info[c["name"]]
        return self.sig_at(c, ("unit",)) and self.dict_ok(c, i["new"][0], i["new"][1], ("unit",), True) \
            and all(self.attrs_ok(c, i[k][0], i[k][1], ("unit",)) for k in ("u2", "u2m"))

    def enums(self, c):
        i = self.info[c["name"]]
        return self.sig_at(c, ("enum",)) and all(self.dict_ok(c, i[k][0], i[k][1], ("enum",), False)
                                                  for k in ("new", "min")) \
            and all(self.attrs_ok(c, i[k][0], i[k][1], ("enum",)) for k in ("e2", "e2b"))

    def required(self, c):
        for pid, data in self.info[c["name"]]["miss"]:
            o = self.obs.get(pid)
            exp = G.v2_from_dict(c, self.cat, data)
            if not o or o["exc"] != ["ValueError", exp[1]]:
                return False
        return bool(self.ok(self.info[c["name"]]["u2"][0]))

    def deprecated(self, c):
        rep = self.byn[c["dep"]]
        sig = self.sigs.get(c["name"])
        if sig != norm(G.v2_sig(rep, self.cat)):
            return False
        i = self.info[c["name"]]
        o, r = self.ok(i["new"][0]), self.ok(i["rep"][0])
        if not o or not r or r["warns"]:
            return False
        return o["warns"] == [["DeprecationWarning", G.dep_message(c)]] and o["isinst"] is True \
            and same(o["dict"], r["dict"]) and same(o["attrs"], r["attrs"])

    def legacy(self, c):
        i = self.info[c["name"]]
        allk = ("str", "int", "float", "bool", "enum", "unit")
        return self.dict_ok(c, i["new"][0], i["new"][1], allk, True) and self.attrs_ok(c, i["r1"][0], i["r1"][1], allk) \
            and self.required(c)

    def imperial(self, c):
        i = self.info[c["name"]]
        return self.sig_at(c, ("unit",)) and self.dict_ok(c, i["new"][0], i["new"][1], ("unit",), True) \
            and all(self.attrs_ok(c, i[k][0], i[k][1], ("unit",)) for k in ("r2", "r1"))

    def enumkeep(self, c):
        return self.enums(c)


def groups(cat):
    g = {}
    exc = set(cat["legacy"]) | set(cat["imperial"]) | set(cat["enumkeep"])
    for s in G.SECTION_NAMES:
        cs = [c for c in cat["classes"] if c["section"] == s and not c["dep"]]

        def has(c, kind):
            return any(f["kind"] == kind for f in c["fields"])
        g["keys/" + s] = ("keys", [c for c in cs if c["name"] not in cat["legacy"]])
        g["read/" + s] = ("read", [c for c in cs if c["name"] not in cat["legacy"]])
        g["compat/" + s] = ("compat", [c for c in cs if c["name"] not in cat["legacy"]])
        g["units/" + s] = ("units", [c for c in cs if has(c, "unit") and c["name"] not in exc])
        g["enums/" + s] = ("enums", [c for c in cs if has(c, "enum") and c["name"] not in exc])
        g["required/" + s] = ("required", [c for c in cs if c["name"] not in cat["legacy"]])
        g["deprecated/" + s] = ("deprecated", [c for c in cat["classes"] if c["section"] == s and c["dep"]])
    byn = G.by_name(cat)
    g["exceptions/legacy-wire"] = ("legacy", [byn[n] for n in cat["legacy"]])
    g["exceptions/imperial"] = ("imperial", [byn[n] for n in cat["imperial"]])
    g["exceptions/enum-keep"] = ("enumkeep", [byn[n] for n in cat["enumkeep"]])
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
        fn, cs = g[item]
        bad = [c["name"] for c in cs if not getattr(j, fn)(c)] if j else ["<no observations>"]
        if bad and os.environ.get("EVO_CHECK_DEBUG"):
            sys.stderr.write("%s: %s\n" % (item, " ".join(bad)))
        print("%s %s" % ("FAIL" if bad else "PASS", item))


if __name__ == "__main__":
    main()
