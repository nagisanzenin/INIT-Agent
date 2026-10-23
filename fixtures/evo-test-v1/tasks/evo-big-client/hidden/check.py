"""evo-big-client check: is client.py migrated as MIGRATION.md says?

Signatures are read statically from client.py (ast). Behaviour is observed by hidden/harness.py, which runs in a
subprocess, imports the workspace's client.py and records the transport calls of probe calls (inputs only).
Expectations come from the generator's model (hidden/apigen.py). One item per (rule, section), plus one per
section for behaviour that must not change, plus one per exception kind. Prints PASS/FAIL <item> lines."""
import ast
import json
import os
import subprocess
import sys

HID = os.path.join(os.environ["INIT_CHECK_DIR"], "hidden")
sys.path.insert(0, HID)
import apigen as G  # noqa: E402

WS = os.getcwd()
SCRATCH = os.environ.get("S1_SCRATCH") or "/tmp"
RULES = ["rename", "placeholders", "deprecated", "pagination", "retries", "fields"]
EXC_ITEMS = ["exceptions/rename-legacy", "exceptions/allow-slash", "exceptions/retries", "exceptions/page-keep",
             "exceptions/raw-body"]
BAD_PH = ["", "x/y", 41, None, 2.5, True, "q/", "/"]
BAD_LIMIT = [0, 201, -3, "20", True, None, 12.0, 1000, False]
ODD_OK = ["0", " sp ace ", "x.y", "-", "%2F"]


def names():
    out = ["%s/%s" % (r, a) for r in RULES for a in G.AREA_NAMES]
    out += ["unchanged/%s" % a for a in G.AREA_NAMES]
    return out + EXC_ITEMS


# ------------------------------------------------------------------------------------------ static


def _lit(node):
    if node is None:
        return None
    try:
        v = ast.literal_eval(node)
    except Exception:  # noqa: BLE001
        return ("expr", ast.dump(node))
    return (type(v).__name__, repr(v))


def static_sigs():
    try:
        with open(os.path.join(WS, "client.py"), "rb") as f:
            tree = ast.parse(f.read().decode("utf-8"))
    except Exception:  # noqa: BLE001
        return {}
    out = {}
    for c in tree.body:
        if not (isinstance(c, ast.ClassDef) and c.name == "Client"):
            continue
        for n in c.body:
            if not isinstance(n, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            a = n.args
            pos = list(getattr(a, "posonlyargs", [])) + list(a.args)
            defs = [None] * (len(pos) - len(a.defaults)) + list(a.defaults)
            sig = []
            for i, x in enumerate(pos[1:], 1):
                sig.append((x.arg, False, defs[i] is not None, _lit(defs[i])))
            for x, d in zip(a.kwonlyargs, a.kw_defaults):
                sig.append((x.arg, True, d is not None, _lit(d)))
            if a.vararg is not None or a.kwarg is not None:
                sig.append(("<var>", None, None, None))
            out[n.name] = sig
    return out


def norm_sig(sig):
    return [(n, kw, hd, (type(d).__name__, repr(d)) if hd else None) for n, kw, hd, d in sig]


# ------------------------------------------------------------------------------------------ probes


class Probes:
    def __init__(self):
        self.list = []

    def add(self, pid, name, kwargs):
        self.list.append({"id": pid, "name": name, "kwargs": kwargs})
        return pid


def ph_v2(m, x, exc):
    return "account_id" if (x == "account_ref" and m["name"] not in exc["legacy"]) else x


def plan(methods, exc):
    """Probe list and, per method, the probe ids and kwargs used."""
    pr = Probes()
    info = {}
    bym = G.by_name(methods)
    for idx, m in enumerate(methods):
        if m["dep"]:
            continue
        salt = 31 * idx + 5
        i = {}
        kw = G.v2_kwargs(m, exc, salt, "h")
        if G.paginated(m, exc):
            kw["cursor"] = "hc%d" % idx
            d = [p["default"] for p in m["params"] if p["role"] == "size"][0]
            kw["limit"] = 1 + (idx * 37) % 199
            if kw["limit"] == d:
                kw["limit"] += 1
        elif m["size"]:
            kw["page"] = 2 + idx % 5
            kw[m["size"]] = 210 + idx % 40
        i["A"] = (pr.add("%s:A" % m["name"], m["name"], kw), kw)
        kwb = G.v2_kwargs(m, exc, salt + 3, "h", full=False)
        i["B"] = (pr.add("%s:B" % m["name"], m["name"], kwb), kwb)
        bad = []
        slash = exc["allow"].get(m["name"])
        for j, x in enumerate(m["ph"]):
            n = ph_v2(m, x, exc)
            vals = [BAD_PH[(idx + j) % len(BAD_PH)], BAD_PH[(idx + j + 3) % len(BAD_PH)]]
            if x == slash:
                vals = ["", 9 + idx % 3]
            for k, v in enumerate(vals):
                if x == slash and isinstance(v, str) and v and v != "/" and "/" in v:
                    continue
                b = dict(kw)
                b[n] = v
                bad.append((pr.add("%s:bad%d.%d" % (m["name"], j, k), m["name"], b), b))
        if len(m["ph"]) >= 2:
            b = dict(kw)
            for j, x in enumerate(m["ph"]):
                b[ph_v2(m, x, exc)] = BAD_PH[(idx + 2 * j + 1) % len(BAD_PH)] if x != slash else ""
            bad.append((pr.add("%s:badall" % m["name"], m["name"], b), b))
        if m["ph"] and G.paginated(m, exc):
            b = dict(kw)
            b[ph_v2(m, m["ph"][0], exc)] = ""
            b["limit"] = 0
            bad.append((pr.add("%s:badorder" % m["name"], m["name"], b), b))
        i["bad"] = bad
        odd = []
        if m["ph"]:
            b = dict(kw)
            for j, x in enumerate(m["ph"]):
                b[ph_v2(m, x, exc)] = ODD_OK[(idx + j) % len(ODD_OK)]
            if slash:
                b[slash] = "deep/%d/x.bin" % idx
            odd.append((pr.add("%s:odd" % m["name"], m["name"], b), b))
            if slash:
                b = dict(kw)
                b[slash] = "a/b"
                odd.append((pr.add("%s:odd2" % m["name"], m["name"], b), b))
        i["odd"] = odd
        lim = []
        if G.paginated(m, exc):
            for k in range(2):
                b = dict(kw)
                b["limit"] = BAD_LIMIT[(idx + 4 * k) % len(BAD_LIMIT)]
                lim.append((pr.add("%s:lim%d" % (m["name"], k), m["name"], b), b))
            b = dict(kw)
            b["limit"] = 200 if idx % 2 else 1
            lim.append((pr.add("%s:limok" % m["name"], m["name"], b), b))
        i["lim"] = lim
        info[m["name"]] = i
    for m in methods:
        if not m["dep"]:
            continue
        r = info[m["dep"]]
        i = {"A": (pr.add("%s:A" % m["name"], m["name"], r["A"][1]), r["A"][1]), "rep": m["dep"]}
        if r["bad"]:
            i["bad"] = (pr.add("%s:bad" % m["name"], m["name"], r["bad"][0][1]), r["bad"][0][0])
        info[m["name"]] = i
    return pr.list, info


def run_harness(probes):
    pin = os.path.join(SCRATCH, "evo-client-probes.json")
    pout = os.path.join(SCRATCH, "evo-client-observed.json")
    with open(pin, "w") as f:
        f.write(json.dumps(probes))
    env = {"PATH": "/usr/bin:/bin", "HOME": SCRATCH, "TMPDIR": SCRATCH, "LC_ALL": "C", "LANG": "C",
           "PYTHONHASHSEED": "0", "PYTHONDONTWRITEBYTECODE": "1", "PYTHONPATH": WS}
    try:
        subprocess.run([sys.executable, "-S", "-s", "-B", os.path.join(HID, "harness.py"), pin, pout, WS],
                       cwd=SCRATCH, env=env, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL, timeout=20)
        with open(pout) as f:
            res = json.load(f)
        return {r["id"]: r for r in res}
    except Exception:  # noqa: BLE001 - a crash or a timeout fails every item
        return {}


# ------------------------------------------------------------------------------------------ judging


def num(x):
    return isinstance(x, (int, float)) and not isinstance(x, bool)


def same(a, b):
    if num(a) and num(b) and (isinstance(a, float) or isinstance(b, float)):
        return isinstance(a, float) == isinstance(b, float) and abs(a - b) <= 1e-9 * max(1.0, abs(b))
    if isinstance(a, dict) and isinstance(b, dict):
        return set(a) == set(b) and all(same(a[k], b[k]) for k in a)
    if isinstance(a, (list, tuple)) and isinstance(b, (list, tuple)):
        return len(a) == len(b) and all(same(x, y) for x, y in zip(a, b))
    return type(a) == type(b) and a == b


def drop(d, keys):
    return dict((k, v) for k, v in d.items() if k not in keys)


class Judge:
    def __init__(self, methods, exc, obs, sigs, info):
        self.methods, self.exc, self.obs, self.sigs, self.info = methods, exc, obs, sigs, info
        self.bym = G.by_name(methods)

    def o(self, pid):
        return self.obs.get(pid)

    def one_call(self, pid):
        r = self.o(pid)
        if not r or r["exc"] or len(r["calls"]) != 1:
            return None
        return r["calls"][0]

    def raised(self, pid, msg):
        r = self.o(pid)
        return bool(r) and r["exc"] == ["ValueError", msg] and r["calls"] == []

    def expect(self, m, kw):
        return G.v2_call(m, kw, self.exc)

    # -- rule items: each returns True when the method satisfies the rule
    def rename(self, m):
        idx = [i for i, p in enumerate(m["params"]) if p["name"] == "account_ref"][0]
        sig = self.sigs.get(m["name"])
        want = norm_sig(G.v2_sig(m, self.exc))
        if sig is None or len(sig) != len(want) or sig[idx] != want[idx]:
            return False
        if any(e[0] == "account_ref" for e in sig):
            return False
        pid, kw = self.info[m["name"]]["A"]
        c = self.one_call(pid)
        if c is None:
            return False
        p = m["params"][idx]
        v = kw["account_id"]
        if "account_ref" in c[2] or "account_ref" in c[3]:
            return False
        if p["role"] == "query":
            return c[2].get("account_id") == v and "account_id" not in c[3]
        if p["role"] == "body":
            return c[3].get("account_id") == v and "account_id" not in c[2]
        return "account_id" not in c[2] and "account_id" not in c[3] and c[1] == self.expect(m, kw)[2]

    def placeholders(self, m):
        i = self.info[m["name"]]
        for pid, kw in i["bad"]:
            e = self.expect(m, kw)
            if e[0] != "raise" or not self.raised(pid, e[2]):
                return False
        for pid, kw in i["odd"]:
            c = self.one_call(pid)
            if c is None or c[1] != self.expect(m, kw)[2]:
                return False
        return True

    def deprecated(self, d):
        sig = self.sigs.get(d["name"])
        if sig != norm_sig(G.v2_sig(d, self.exc)):
            return False
        i = self.info[d["name"]]
        rep = self.info[i["rep"]]
        msg = [["DeprecationWarning", G.dep_message(d)]]
        a, ra = self.o(i["A"][0]), self.o(rep["A"][0])
        if not a or not ra or a["warns"] != msg or a["exc"] or not a["ret"] or ra["exc"]:
            return False
        if not same(a["calls"], ra["calls"]) or len(a["calls"]) != 1:
            return False
        if "bad" in i:
            b, rb = self.o(i["bad"][0]), self.o(i["bad"][1])
            if not b or not rb or b["warns"] != msg or b["exc"] != rb["exc"] or not same(b["calls"], rb["calls"]):
                return False
        return True

    def pagination(self, m):
        sig = self.sigs.get(m["name"])
        want = norm_sig(G.v2_sig(m, self.exc))
        if sig is None or len(sig) != len(want):
            return False
        for j, p in enumerate(m["params"]):
            if p["role"] in ("page", "size") and sig[j] != want[j]:
                return False
        if any(e[0] in ("page", "per_page", "page_size") for e in sig):
            return False
        i = self.info[m["name"]]
        for key in ("A", "B"):
            pid, kw = i[key]
            c = self.one_call(pid)
            e = self.expect(m, kw)
            if c is None:
                return False
            got = dict((k, v) for k, v in c[2].items() if k in G.PAGINATION_NAMES)
            want_q = dict((k, v) for k, v in e[3].items() if k in G.PAGINATION_NAMES)
            if not same(got, want_q):
                return False
        for pid, kw in i["lim"]:
            e = self.expect(m, kw)
            if e[0] == "raise":
                if not self.raised(pid, e[2]):
                    return False
            else:
                c = self.one_call(pid)
                if c is None or c[2].get("limit") != kw["limit"]:
                    return False
        return True

    def retries(self, m):
        c = self.one_call(self.info[m["name"]]["A"][0])
        return c is not None and same(c[4], G.retries_of(m, self.exc))

    def fields(self, m):
        for key in ("A", "B"):
            pid, kw = self.info[m["name"]][key]
            c = self.one_call(pid)
            if c is None:
                return False
            e = self.expect(m, kw)
            if not same(drop(c[3], G.ACCOUNT_NAMES), drop(e[4], G.ACCOUNT_NAMES)):
                return False
        return True

    def unchanged(self, m):
        sig = self.sigs.get(m["name"])
        if sig is None:
            return False
        skip = G.ACCOUNT_NAMES + G.PAGINATION_NAMES
        keep = [e for e in sig if e[0] not in skip]
        if keep != [e for e in norm_sig(G.v1_sig(m)) if e[0] not in skip]:
            return False
        if len(sig) != len(m["params"]):
            return False
        for key in ("A", "B"):
            pid, kw = self.info[m["name"]][key]
            c = self.one_call(pid)
            if c is None:
                return False
            e = self.expect(m, kw)
            bkeys = G.ACCOUNT_NAMES + G.MAPPED + tuple(G.NEW_KEY.values())
            if c[0] != e[1] or c[1] != e[2] or not same(drop(c[2], skip), drop(e[3], skip)) \
                    or not same(drop(c[3], bkeys), drop(e[4], bkeys)) or not self.o(pid)["ret"]:
                return False
        return True

    # -- exception items
    def ex_legacy(self, m):
        idx = [i for i, p in enumerate(m["params"]) if p["name"] == "account_ref"][0]
        sig = self.sigs.get(m["name"])
        if sig is None or len(sig) <= idx or sig[idx] != norm_sig(G.v1_sig(m))[idx]:
            return False
        if any(e[0] == "account_id" for e in sig):
            return False
        pid, kw = self.info[m["name"]]["A"]
        c = self.one_call(pid)
        if c is None or "account_id" in c[2] or "account_id" in c[3]:
            return False
        e = self.expect(m, kw)
        return same(dict((k, v) for k, v in c[2].items() if k == "account_ref"),
                    dict((k, v) for k, v in e[3].items() if k == "account_ref")) and \
            same(dict((k, v) for k, v in c[3].items() if k == "account_ref"),
                 dict((k, v) for k, v in e[4].items() if k == "account_ref")) and c[1] == e[2]

    def ex_allow(self, m):
        return self.placeholders(m)

    def ex_retries(self, m):
        return self.retries(m)

    def ex_keep(self, m):
        sig = self.sigs.get(m["name"])
        want = norm_sig(G.v1_sig(m))
        if sig is None or len(sig) != len(want):
            return False
        for j, p in enumerate(m["params"]):
            if p["role"] in ("page", "size") and sig[j] != want[j]:
                return False
        for key in ("A", "B"):
            pid, kw = self.info[m["name"]][key]
            c = self.one_call(pid)
            if c is None:
                return False
            e = self.expect(m, kw)
            got = dict((k, v) for k, v in c[2].items() if k in G.PAGINATION_NAMES)
            if not same(got, dict((k, v) for k, v in e[3].items() if k in G.PAGINATION_NAMES)):
                return False
        return True

    def ex_raw(self, m):
        return self.fields(m)


def groups(methods, exc):
    """item name -> (judge method name, [methods])"""
    special = set(exc["legacy"]) | set(exc["allow"]) | set(exc["retries"]) | set(exc["keep"]) | set(exc["raw"])
    g = {}
    for a in G.AREA_NAMES:
        ms = [m for m in methods if m["area"] == a]
        live = [m for m in ms if not m["dep"]]
        g["rename/" + a] = ("rename", [m for m in live if G.renamed(m, exc)])
        g["placeholders/" + a] = ("placeholders", [m for m in live if m["ph"] and m["name"] not in exc["allow"]])
        g["deprecated/" + a] = ("deprecated", [m for m in ms if m["dep"]])
        g["pagination/" + a] = ("pagination", [m for m in live if G.paginated(m, exc)])
        g["retries/" + a] = ("retries", [m for m in live if m["name"] not in exc["retries"]])
        g["fields/" + a] = ("fields", [m for m in live if m["name"] not in exc["raw"] and any(
            p["role"] == "body" and p["name"] in G.MAPPED for p in m["params"])])
        g["unchanged/" + a] = ("unchanged", live)
    bym = G.by_name(methods)
    g["exceptions/rename-legacy"] = ("ex_legacy", [bym[n] for n in exc["legacy"]])
    g["exceptions/allow-slash"] = ("ex_allow", [bym[n] for n in sorted(exc["allow"])])
    g["exceptions/retries"] = ("ex_retries", [bym[n] for n in sorted(exc["retries"])])
    g["exceptions/page-keep"] = ("ex_keep", [bym[n] for n in exc["keep"]])
    g["exceptions/raw-body"] = ("ex_raw", [bym[n] for n in exc["raw"]])
    assert all(len(v[1]) >= 2 for v in g.values()), [k for k, v in g.items() if len(v[1]) < 2]
    del special
    return g


def main():
    methods, exc = G.build()
    probes, info = plan(methods, exc)
    sigs = static_sigs()
    obs = run_harness(probes) if sigs else {}
    j = Judge(methods, exc, obs, sigs, info)
    g = groups(methods, exc)
    for item in names():
        fn, ms = g[item]
        bad = [m["name"] for m in ms if not getattr(j, fn)(m)] if obs else ["<no observations>"]
        if bad and os.environ.get("EVO_CHECK_DEBUG"):
            sys.stderr.write("%s: %s\n" % (item, " ".join(bad)))
        print("%s %s" % ("FAIL" if bad else "PASS", item))


if __name__ == "__main__":
    main()
