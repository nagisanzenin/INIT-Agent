"""evo-big-client: generator of the initial workspace, and the behaviour model the check uses.

As a script (`apigen.py WS`, run by workspace.gen) it writes WS/client.py (the generated v1 client),
WS/MIGRATION.md (the v1 -> v2 rules and the exceptions table) and WS/tests/test_client.py (visible examples).
Imported, build() returns (methods, exceptions) and the v2_* functions give the migrated behaviour.
Deterministic: one seeded random.Random, no clock, no environment. Python 3.9, stdlib only.
"""
import os
import random
import sys

SEED = 7310
MAX_LIMIT = 200
RETRIES = 3
MAPPED = ("weight_lb", "length_in", "temp_f", "qty", "desc")
NEW_KEY = {"weight_lb": "weight_kg", "length_in": "length_cm", "temp_f": "temp_c", "qty": "quantity",
           "desc": "description"}
NUM = {"weight_lb": "float", "length_in": "float", "temp_f": "float", "qty": "int", "amount": "int",
       "priority": "int", "seats": "int"}
PAGINATION_NAMES = ("page", "per_page", "page_size", "cursor", "limit")
ACCOUNT_NAMES = ("account_ref", "account_id")

# area, nouns (first is the main resource), (parent, child) for nested templates, body fields, query filters
AREAS = [
    ("accounts", ["account", "member", "invite", "api_key", "webhook"], ("member", "grant"),
     ["name", "email", "role", "note", "plan_code", "seats", "desc"], ["status", "role", "since", "q"]),
    ("orders", ["order", "refund", "hold", "quote", "return"], ("order", "line"),
     ["note", "currency", "channel", "qty", "desc", "weight_lb", "priority"], ["status", "since", "until", "channel"]),
    ("catalog", ["product", "category", "bundle", "price_list", "brand"], ("product", "variant"),
     ["title", "sku", "desc", "weight_lb", "length_in", "color", "qty"], ["q", "tag", "brand", "status"]),
    ("shipping", ["shipment", "carrier", "pickup", "manifest", "zone"], ("shipment", "parcel"),
     ["carrier_code", "weight_lb", "length_in", "temp_f", "note", "priority", "desc"], ["status", "carrier", "since", "region"]),
    ("billing", ["invoice", "payment", "credit_note", "plan", "coupon"], ("invoice", "item"),
     ["amount", "currency", "due_date", "note", "desc", "qty", "reason"], ["status", "since", "until", "currency"]),
    ("inventory", ["warehouse", "stock_item", "transfer", "adjustment", "supplier"], ("warehouse", "bin"),
     ["sku", "qty", "temp_f", "bin_code", "reason", "weight_lb", "desc"], ["sku", "status", "since", "region"]),
    ("users", ["user", "role", "group", "token", "device"], ("user", "session"),
     ["name", "email", "locale", "desc", "note", "timezone", "role"], ["q", "status", "role", "since"]),
    ("reports", ["report", "export", "schedule", "dashboard", "dataset"], ("report", "run"),
     ["title", "format", "schedule", "desc", "timezone", "note", "priority"], ["kind", "since", "until", "owner"]),
]
AREA_NAMES = [a[0] for a in AREAS]
ACTIONS = ["archive", "restore", "approve", "cancel", "sync", "lock", "verify", "close"]
SIZE_DEFAULTS = [10, 20, 25, 50, 100]
# allow-slash specials: area -> (name, parent noun, literal segment, slash parameter)
SPECIALS = {
    "catalog": ("get_product_asset", "product", "assets", "asset_key"),
    "users": ("get_user_avatar", "user", "avatars", "avatar_path"),
    "reports": ("get_export_file", "export", "files", "file_key"),
    "inventory": ("get_warehouse_bin_label", "warehouse", "labels", "label_path"),
}
DEP_PREFIX = {"get": "fetch", "list": "all", "create": "new", "update": "modify", "replace": "overwrite",
              "delete": "remove"}


def plural(n):
    return n + "s"


def P(name, role, req, default=None, kw=False):
    return {"name": name, "role": role, "req": req, "default": default, "kw": kw,
            "type": NUM.get(name, "bool" if role == "qflag" else "str")}


def _coll(area, noun):
    if area == "accounts":
        if noun == "account":
            return "/v1/accounts", "account_ref"
        return "/v1/accounts/{account_ref}/" + plural(noun), noun + "_id"
    return "/v1/%s/%s" % (area, plural(noun)), noun + "_id"


def _placeholders(path):
    out, i = [], 0
    while True:
        a = path.find("{", i)
        if a < 0:
            return out
        b = path.index("}", a)
        out.append(path[a + 1:b])
        i = b + 1


def _make(rng, area, kind, noun, fields, filters, child=None):
    coll, idp = _coll(area, noun)
    item = coll + "/{" + idp + "}"
    verb = {"get": "GET", "list": "GET", "create": "POST", "update": "PATCH", "replace": "PUT",
            "delete": "DELETE", "action": "POST", "list_child": "GET", "get_child": "GET"}[kind]
    act = None
    if kind == "get":
        name, path = "get_" + noun, item
    elif kind == "list":
        name, path = "list_" + plural(noun), coll
    elif kind == "create":
        name, path = "create_" + noun, coll
    elif kind == "update":
        name, path = "update_" + noun, item
    elif kind == "replace":
        name, path = "replace_" + noun, item
    elif kind == "delete":
        name, path = "delete_" + noun, item
    elif kind == "action":
        act = rng.choice(ACTIONS)
        name, path = act + "_" + noun, item + "/" + act
    elif kind == "list_child":
        name, path = "list_%s_%s" % (noun, plural(child)), item + "/" + plural(child)
    else:
        name, path = "get_%s_%s" % (noun, child), item + "/" + plural(child) + "/{" + child + "_id}"
    ph = _placeholders(path)
    req, opt = [P(x, "path", True) for x in ph], []
    has_acct = "account_ref" in ph
    want_acct = (not has_acct) and rng.random() < 0.34
    flt = list(filters)
    rng.shuffle(flt)
    fl = list(fields)
    rng.shuffle(fl)
    if kind in ("get", "get_child"):
        for f in flt[:rng.choice([0, 1, 1, 2])]:
            opt.append(P(f, "query", False))
        if rng.random() < 0.3:
            opt.append(P("expand", "query", False))
        if want_acct:
            opt.insert(rng.randint(0, len(opt)), P("account_ref", "query", False))
    elif kind in ("list", "list_child"):
        for f in flt[:rng.choice([1, 1, 2, 3])]:
            opt.append(P(f, "query", False))
        if want_acct:
            if rng.random() < 0.3:
                req.append(P("account_ref", "query", True))
            else:
                opt.insert(rng.randint(0, len(opt)), P("account_ref", "query", False))
        if rng.random() < 0.35:
            opt.append(P("desc", "qflag", False, False))
    elif kind in ("create", "replace"):
        nreq = rng.choice([1, 2]) if kind == "create" else rng.choice([2, 2, 3])
        nopt = rng.choice([1, 2, 3]) if kind == "create" else rng.choice([0, 1, 1, 2])
        for f in fl[:nreq]:
            req.append(P(f, "body", True))
        for f in fl[nreq:nreq + nopt]:
            opt.append(P(f, "body", False))
        if want_acct:
            if rng.random() < 0.5:
                req.insert(rng.randint(len(ph), len(req)), P("account_ref", "body", True))
            else:
                opt.insert(rng.randint(0, len(opt)), P("account_ref", "body", False))
        if kind == "create" and rng.random() < 0.2:
            opt.append(P("dry_run", "qflag", False, False))
    elif kind == "update":
        nreq = rng.choice([0, 0, 1])
        for f in fl[:nreq]:
            req.append(P(f, "body", True))
        for f in fl[nreq:nreq + rng.choice([1, 2, 3])]:
            opt.append(P(f, "body", False))
        if want_acct:
            opt.insert(rng.randint(0, len(opt)), P("account_ref", "body", False))
    elif kind == "delete":
        if want_acct and rng.random() < 0.7:
            opt.append(P("account_ref", "query", False))
        if rng.random() < 0.4:
            opt.append(P("force", "qflag", False, False))
    else:  # action
        pool = [f for f in fl if f in ("note", "reason", "qty", "desc", "priority")] or fl
        for f in pool[:rng.choice([0, 1, 1, 2])]:
            opt.append(P(f, "body", False))
        if want_acct and rng.random() < 0.5:
            opt.append(P("account_ref", "query", False))
    size = None
    if kind in ("list", "list_child"):
        size = rng.choice(["per_page", "per_page", "page_size"])
        opt.append(P("page", "page", False, 1))
        opt.append(P(size, "size", False, rng.choice(SIZE_DEFAULTS)))
    if opt and rng.random() < 0.25:
        for p in opt:
            p["kw"] = True
    idem_p = {"GET": 0.7, "PUT": 0.8, "DELETE": 0.6, "PATCH": 0.15, "POST": 0.1}[verb]
    return {"name": name, "area": area, "kind": kind, "noun": noun, "verb": verb, "path": path, "ph": ph,
            "params": req + opt, "idem": rng.random() < idem_p, "dep": None, "size": size, "act": act}


def _special(area):
    name, parent, seg, slash = SPECIALS[area]
    coll, idp = _coll(area, parent)
    path = coll + "/{" + idp + "}/" + seg + "/{" + slash + "}"
    ph = _placeholders(path)
    return {"name": name, "area": area, "kind": "get_child", "noun": parent, "verb": "GET", "path": path,
            "ph": ph, "params": [P(x, "path", True) for x in ph], "idem": True, "dep": None, "size": None,
            "act": None}


def _deprecated(rng, rep):
    k = rep["kind"]
    if k in DEP_PREFIX:
        name = DEP_PREFIX[k] + rep["name"][rep["name"].index("_"):]
    elif k == "list_child":
        name = "all" + rep["name"][4:]
    elif k == "get_child":
        name = "fetch" + rep["name"][3:]
    else:
        name = "do_" + rep["name"]
    params = [dict(p) for p in rep["params"]]
    return {"name": name, "area": rep["area"], "kind": rep["kind"], "noun": rep["noun"], "verb": rep["verb"],
            "path": "/v0" + rep["path"][3:], "ph": list(rep["ph"]), "params": params,
            "idem": rng.random() < 0.5, "dep": rep["name"], "size": rep["size"], "act": rep["act"]}


def build():
    """(methods, exceptions); methods in file order, exceptions = {legacy, allow, retries, keep, raw}."""
    rng = random.Random(SEED)
    sections = []
    for area, nouns, (cparent, cchild), fields, filters in AREAS:
        cands = []
        for noun in nouns:
            kinds = ["get", "list", "create", "update", "delete"]
            if rng.random() < 0.5:
                kinds.append("replace")
            if rng.random() < 0.6:
                kinds.append("action")
            for k in kinds:
                cands.append(_make(rng, area, k, noun, fields, filters))
        cands.append(_make(rng, area, "list_child", cparent, fields, filters, cchild))
        cands.append(_make(rng, area, "get_child", cparent, fields, filters, cchild))
        seen, uniq = set(), []
        for m in cands:
            if m["name"] not in seen:
                seen.add(m["name"])
                uniq.append(m)
        keep_lists = [m for m in uniq if m["kind"] in ("list", "list_child")]
        others = [m for m in uniq if m["kind"] not in ("list", "list_child")]
        rng.shuffle(others)
        regular = keep_lists + others[:27 - len(keep_lists)]
        if area in SPECIALS:
            regular.append(_special(area))
        sections.append((area, regular))
    # deprecated methods: 3 per area, each a copy of a regular method of the same area
    exc = {"legacy": [], "allow": {}, "retries": {}, "keep": [], "raw": []}
    reserved = set()
    out = []
    for area, regular in sections:
        pool = [m for m in regular if m["name"] not in [s[0] for s in SPECIALS.values()]]
        reps = rng.sample(pool, 3)
        deps = []
        for rep in reps:
            reserved.add(rep["name"])
            deps.append(_deprecated(rng, rep))
        allm = regular + deps
        rng.shuffle(allm)
        out.append((area, allm))
    methods = [m for _, ms in out for m in ms]
    names = [m["name"] for m in methods]
    assert len(names) == len(set(names)), "duplicate method names"
    free = [m for m in methods if not m["dep"] and m["name"] not in reserved
            and m["name"] not in [s[0] for s in SPECIALS.values()]]
    used = set()

    def pick(cond, n, spread=True):
        got, areas = [], set()
        for m in free:
            if m["name"] in used or not cond(m):
                continue
            if spread and m["area"] in areas:
                continue
            got.append(m)
            areas.add(m["area"])
            used.add(m["name"])
            if len(got) == n:
                break
        assert len(got) == n, "not enough candidates"
        return got

    rng.shuffle(free)
    for m in pick(lambda m: any(p["name"] == "account_ref" for p in m["params"]), 6):
        exc["legacy"].append(m["name"])
    for area, (name, parent, seg, slash) in sorted(SPECIALS.items()):
        exc["allow"][name] = slash
    for m, n in zip(pick(lambda m: m["idem"], 4), [5, 1, 0, 5]):
        exc["retries"][m["name"]] = n
    for m, n in zip(pick(lambda m: not m["idem"], 2), [2, 1]):
        exc["retries"][m["name"]] = n
    for m in pick(lambda m: m["size"] is not None and "account_ref" not in [p["name"] for p in m["params"]], 4):
        exc["keep"].append(m["name"])
    for m in pick(lambda m: any(p["role"] == "body" and p["name"] in MAPPED for p in m["params"])
                  and "account_ref" not in [p["name"] for p in m["params"]], 5):
        exc["raw"].append(m["name"])
    exc["legacy"].sort()
    exc["keep"].sort()
    exc["raw"].sort()
    return methods, exc


# ------------------------------------------------------------------------------------------ model


def by_name(methods):
    return {m["name"]: m for m in methods}


def renamed(m, exc):
    return any(p["name"] == "account_ref" for p in m["params"]) and m["name"] not in exc["legacy"]


def paginated(m, exc):
    return m["size"] is not None and m["name"] not in exc["keep"]


def v2_name(m, p, exc):
    if p["name"] == "account_ref" and m["name"] not in exc["legacy"]:
        return "account_id"
    if p["role"] == "page" and m["name"] not in exc["keep"]:
        return "cursor"
    if p["role"] == "size" and m["name"] not in exc["keep"]:
        return "limit"
    return p["name"]


def v1_sig(m):
    return [(p["name"], p["kw"], not p["req"], None if p["req"] else p["default"]) for p in m["params"]]


def v2_sig(m, exc):
    out = []
    for p in m["params"]:
        n = v2_name(m, p, exc)
        d = p["default"]
        if n == "cursor":
            d = None
        out.append((n, p["kw"], not p["req"], None if p["req"] else d))
    return out


def retries_of(m, exc):
    if m["name"] in exc["retries"]:
        return exc["retries"][m["name"]]
    return RETRIES if m["idem"] else 0


def _valid_ph(v, slash_ok):
    return isinstance(v, str) and v != "" and (slash_ok or "/" not in v)


def _valid_limit(v):
    return isinstance(v, int) and not isinstance(v, bool) and 1 <= v <= MAX_LIMIT


def map_value(k, v):
    if k == "weight_lb":
        return round(v * 0.45359237, 3)
    if k == "length_in":
        return round(v * 2.54, 2)
    if k == "temp_f":
        return round((v - 32) * 5 / 9, 1)
    return v


def _subst(path, values):
    for k, v in values.items():
        path = path.replace("{" + k + "}", v)
    return path


def v1_call(m, kw):
    """The v1 behaviour for kwargs in v1 names: ("call", verb, path, params, json, retries)."""
    vals = {}
    for p in m["params"]:
        vals[p["name"]] = kw[p["name"]] if p["name"] in kw else p["default"]
    path = _subst(m["path"], dict((x, vals[x]) for x in m["ph"]))
    params, body, has_body = {}, {}, False
    for p in m["params"]:
        v = vals[p["name"]]
        if p["role"] in ("query", "page", "size"):
            if p["req"] or p["role"] in ("page", "size") or v is not None:
                params[p["name"]] = v
        elif p["role"] == "qflag":
            if v:
                params[p["name"]] = "true"
        elif p["role"] == "body":
            has_body = True
            if p["req"] or v is not None:
                body[p["name"]] = v
    return ("call", m["verb"], path, params, body if has_body else {}, 0)


def v2_call(m, kw, exc):
    """The v2 behaviour of a non-deprecated method for kwargs in v2 names: ("call", verb, path, params, json,
    retries) or ("raise", "ValueError", message)."""
    assert not m["dep"]
    name = m["name"]
    vals = {}
    for p in m["params"]:
        n = v2_name(m, p, exc)
        d = None if n == "cursor" else p["default"]
        vals[n] = kw[n] if n in kw else d
    for x in m["ph"]:
        n = "account_id" if (x == "account_ref" and name not in exc["legacy"]) else x
        if not _valid_ph(vals[n], exc["allow"].get(name) == x):
            return ("raise", "ValueError", "%s: invalid %s: %r" % (name, n, vals[n]))
    pag = paginated(m, exc)
    if pag and not _valid_limit(vals["limit"]):
        return ("raise", "ValueError", "%s: invalid limit: %r" % (name, vals["limit"]))
    path = m["path"]
    for x in m["ph"]:
        n = "account_id" if (x == "account_ref" and name not in exc["legacy"]) else x
        path = path.replace("{" + x + "}", vals[n])
    params, body = {}, {}
    raw = name in exc["raw"]
    for p in m["params"]:
        n = v2_name(m, p, exc)
        v = vals[n]
        wire = n if p["name"] == "account_ref" else p["name"]
        if p["role"] == "query":
            if p["req"] or v is not None:
                params[wire] = v
        elif p["role"] == "qflag":
            if v:
                params[wire] = "true"
        elif p["role"] in ("page", "size"):
            if not pag:
                params[wire] = v
            elif n == "limit":
                params["limit"] = v
            elif v is not None:
                params["cursor"] = v
        elif p["role"] == "body":
            if p["req"] or v is not None:
                if wire in MAPPED and not raw:
                    body[NEW_KEY[wire]] = map_value(wire, v)
                else:
                    body[wire] = v
    return ("call", m["verb"], path, params, body, retries_of(m, exc))


def dep_message(m):
    return "%s() is deprecated; use %s() instead" % (m["name"], m["dep"])


def sample_value(p, salt, tag):
    """A valid argument value for parameter p (v1 or v2 view), deterministic in salt."""
    t = p["type"]
    if p["role"] == "qflag":
        return True
    if t == "float":
        return round(3.25 + (salt * 7.5) % 61, 2) if p["name"] != "temp_f" else round(14.0 + (salt * 9.3) % 90, 1)
    if t == "int":
        return 1 + (salt * 13) % 40
    return "%s%d-%s" % (tag, salt, p["name"].split("_")[0][:3])


# ------------------------------------------------------------------------------------------ render v1


HEADER = '''"""Harbor API client (v1 surface).

Generated from the Harbor service catalogue. Every public method of `Client` wraps one HTTP endpoint and calls
the transport exactly once:

    self._transport.request(verb, path, params=None, json=None, retries=0)

`params` is the query string (a dict), `json` the request body (a dict) and `retries` how many times the
transport may retry the request after a failure. A method returns whatever the transport returns.

The `@endpoint` decorator records the verb, the path template and the flags of an endpoint; `@timed` and `@beta`
only attach a metric name and a label. MIGRATION.md describes the move to the v2 surface.
"""
import functools


def endpoint(verb=None, path=None, idempotent=False, **meta):
    """Record the endpoint of a client method (metadata only; the behaviour lives in the method)."""
    def deco(fn):
        fn.__endpoint__ = {"verb": verb, "path": path, "idempotent": idempotent, "meta": dict(meta)}
        return fn
    return deco


def timed(metric):
    """Attach a metric name to a method; the wrapper only forwards the call."""
    def deco(fn):
        @functools.wraps(fn)
        def wrapper(*args, **kwargs):
            return fn(*args, **kwargs)
        wrapper.__metric__ = metric
        return wrapper
    return deco


def beta(fn):
    """Label a method as beta (metadata only)."""
    fn.__beta__ = True
    return fn


class Client:
    """Harbor API client: one method per endpoint."""

    def __init__(self, transport):
        self._transport = transport
'''

SUMMARY = {
    "get": "Fetch one {n}.", "list": "List {ns}.", "create": "Create {an} {n}.", "update": "Update fields of {an} {n}.",
    "replace": "Replace {an} {n} as a whole.", "delete": "Delete {an} {n}.", "action": "{A} {an} {n}.",
    "list_child": "List the {c}s of {an} {n}.", "get_child": "Fetch one {c} of {an} {n}.",
}
ARGDOC = {
    "path": "identifier in the path.", "query": "optional filter.", "qflag": "send the flag when true.",
    "body": "request body field.", "page": "page number, starting at 1.", "size": "number of items per page.",
}
NOTES = ["# server ignores unknown filters", "# keep in sync with the service catalogue",
         "# the service treats a missing key as unset", "# see the API changelog for this endpoint"]


def _endpoint_lines(m, rng):
    flag = None
    if m["idem"]:
        flag = "idempotent=True"
    elif rng.random() < 0.4:
        flag = "idempotent=False"
    style = rng.choice(["pos", "pos", "pos", "kw", "multi"])
    if style == "kw":
        args = ['verb="%s"' % m["verb"], 'path="%s"' % m["path"]]
    else:
        args = ['"%s"' % m["verb"], '"%s"' % m["path"]]
    extra = [flag] if flag else []
    if rng.random() < 0.2:
        extra.insert(rng.randint(0, len(extra)), 'tags=("%s",)' % m["area"])
    args += extra
    if style == "multi":
        return ["@endpoint("] + ["    %s," % a for a in args] + [")"]
    return ["@endpoint(%s)" % ", ".join(args)]


def _sig_lines(m, rng):
    parts = ["self"]
    star = False
    for p in m["params"]:
        if p["kw"] and not star:
            parts.append("*")
            star = True
        parts.append(p["name"] if p["req"] else "%s=%r" % (p["name"], p["default"]))
    one = "def %s(%s):" % (m["name"], ", ".join(parts))
    r = rng.random()
    if len(one) <= 92 and r < 0.65:
        return [one]
    if r < 0.82:
        head = "def %s(" % m["name"]
        lines, cur = [], head
        for i, part in enumerate(parts):
            piece = part + (", " if i < len(parts) - 1 else "):")
            if len(cur) + len(piece) > 92 and cur.strip() != head.strip():
                lines.append(cur.rstrip())
                cur = " " * len(head) + piece
            else:
                cur += piece
        lines.append(cur)
        return lines
    return ["def %s(" % m["name"]] + ["    %s," % x for x in parts] + ["):"]


def _doc_lines(m, rng):
    s = SUMMARY[m["kind"]].format(n=m["noun"].replace("_", " "), ns=plural(m["noun"]).replace("_", " "),
                                  c=(m["path"].split("/")[-2][:-1] if m["kind"] == "get_child" else
                                     m["path"].split("/")[-1][:-1]),
                                  A=(m["act"] or "").capitalize(), an="an" if m["noun"][0] in "aeiou" else "a")
    r = rng.random()
    if r < 0.05:
        return []
    if r < 0.25 or not m["params"]:
        return ['"""%s"""' % s]
    out = ['"""%s' % s, "", "Args:"]
    for p in m["params"]:
        d = "the owning account." if p["name"] == "account_ref" else ARGDOC[p["role"]]
        out.append("    %s: %s" % (p["name"], d))
    if rng.random() < 0.6:
        out += ["", "Returns:", "    The decoded response of the transport."]
    out.append('"""')
    return out


def _path_expr(m, rng):
    path, ph = m["path"], m["ph"]
    if not ph:
        return '"%s"' % path
    style = rng.choice(["fmt_kw", "fmt_pos", "fstr", "pct", "concat"])
    if style == "fmt_kw":
        return '"%s".format(%s)' % (path, ", ".join("%s=%s" % (x, x) for x in ph))
    if style == "fstr":
        return 'f"%s"' % path
    tmpl = path
    for x in ph:
        tmpl = tmpl.replace("{" + x + "}", "{}" if style == "fmt_pos" else "%s")
    if style == "fmt_pos":
        return '"%s".format(%s)' % (tmpl, ", ".join(ph))
    if style == "pct":
        return '"%s" %% (%s)' % (tmpl, ", ".join(ph) + ("," if len(ph) == 1 else ""))
    pieces, rest = [], path
    for x in ph:
        a = rest.index("{" + x + "}")
        if a:
            pieces.append('"%s"' % rest[:a])
        pieces.append(x)
        rest = rest[a + len(x) + 2:]
    if rest:
        pieces.append('"%s"' % rest)
    return " + ".join(pieces)


def _dict_lines(var, fixed, cond, flags, rng):
    """fixed: [(key, expr)] always set; cond: [(key, name)] set when not None; flags: [(key, name)] -> "true"."""
    style = rng.choice(["lit", "dict", "inc"])
    out = []
    if style == "lit":
        out.append("%s = {%s}" % (var, ", ".join('"%s": %s' % kv for kv in fixed)))
    elif style == "dict":
        out.append("%s = dict(%s)" % (var, ", ".join("%s=%s" % kv for kv in fixed)))
    else:
        out.append("%s = {}" % var)
        for k, e in fixed:
            out.append('%s["%s"] = %s' % (var, k, e))
    cond_lines = []
    for k, n in cond:
        cond_lines.append(["if %s is not None:" % n, '    %s["%s"] = %s' % (var, k, n)])
    for k, n in flags:
        cond_lines.append(["if %s:" % n, '    %s["%s"] = "true"' % (var, k)])
    rng.shuffle(cond_lines)
    for c in cond_lines:
        out += c
    return out


def _body_lines(m, rng):
    out = []
    pexpr = _path_expr(m, rng)
    inline = rng.random() < 0.25
    if not inline:
        out.append("path = " + pexpr)
    q = [p for p in m["params"] if p["role"] in ("query", "qflag", "page", "size")]
    b = [p for p in m["params"] if p["role"] == "body"]
    if rng.random() < 0.1:
        out.append(rng.choice(NOTES))
    if q:
        fixed = [(p["name"], p["name"]) for p in q if p["role"] in ("page", "size") or (p["role"] == "query" and p["req"])]
        cond = [(p["name"], p["name"]) for p in q if p["role"] == "query" and not p["req"]]
        flags = [(p["name"], p["name"]) for p in q if p["role"] == "qflag"]
        out += _dict_lines("params", fixed, cond, flags, rng)
    if b:
        fixed = [(p["name"], p["name"]) for p in b if p["req"]]
        cond = [(p["name"], p["name"]) for p in b if not p["req"]]
        out += _dict_lines("body", fixed, cond, [], rng)
    args = ['"%s"' % m["verb"], pexpr if inline else "path"]
    if q:
        args.append("params=params")
    if b:
        args.append("json=body")
    style = rng.choice(["ret", "ret", "ret", "multi", "resp"])
    call = "self._transport.request(%s)" % ", ".join(args)
    if style == "multi" or len(call) > 84:
        lines = ["self._transport.request("] + ["    %s," % a for a in args] + [")"]
        if style == "resp":
            out += ["resp = " + lines[0]] + lines[1:] + ["return resp"]
        else:
            out += ["return " + lines[0]] + lines[1:]
    elif style == "resp":
        out += ["resp = " + call, "return resp"]
    else:
        out.append("return " + call)
    return out


def render_method(m, rng):
    block = [_endpoint_lines(m, rng)]
    if rng.random() < 0.6:
        block.append(['@timed("%s.%s")' % (m["area"], m["name"])])
    if rng.random() < 0.12:
        block.append(["@beta"])
    rng.shuffle(block)
    if rng.random() < 0.15:
        block.insert(rng.randint(0, len(block)), ["# owner: %s-team" % m["area"]])
    if m["dep"]:
        block.insert(rng.randint(0, len(block)), ["# deprecated: use %s" % m["dep"]])
    lines = [""]
    for el in block:
        lines += ["    " + x for x in el]
    lines += ["    " + x for x in _sig_lines(m, rng)]
    lines += [("        " + x) if x else "" for x in _doc_lines(m, rng)]
    lines += ["        " + x for x in _body_lines(m, rng)]
    return lines


def render_client(methods):
    rng = random.Random(SEED + 1)
    lines = HEADER.rstrip("\n").split("\n")
    area = None
    for m in methods:
        if m["area"] != area:
            area = m["area"]
            lines += ["", "    # " + "-" * 60 + " " + area]
        lines += render_method(m, rng)
    return "\n".join(lines) + "\n"


# ------------------------------------------------------------------------------------------ MIGRATION.md


MIGRATION = """# Harbor client: v1 to v2 migration

`client.py` holds the generated v1 client: class `Client`, one method per endpoint (%(count)d methods in eight
sections: %(areas)s). The generator is gone, so the file is migrated in place. Apply the six rules below to
every method of `Client`, except where the exceptions table at the end says otherwise. Anything this document
does not change keeps working exactly as before: the same methods, HTTP verbs and paths, the same query
parameters and body keys, the same parameter names, order, defaults and keyword-only markers, and the same
return value (whatever the transport returns).

## Terms

- **Transport call**: each method calls `self._transport.request(verb, path, params=None, json=None, retries=0)`
  once. `params` is the query (a dict), `json` the request body (a dict). Omitted or `None` `params`/`json` mean
  an empty query / an empty body.
- **Signature**: the method's `def` in `client.py` is the signature that counts. Every method stays a plain
  `def` inside `class Client` whose parameter list states its v2 parameters directly (names, positions,
  defaults, `*` markers): signatures are read from the source, and the decorators stay as they are.
- **Path placeholders**: the `{name}` segments of the path given to the method's `@endpoint(...)` decorator.
  Each one is a parameter of the method.
- **Idempotent**: the method's `@endpoint(...)` decorator passes `idempotent=True`. A missing flag or
  `idempotent=False` means not idempotent. The HTTP verb does not matter.
- **Deprecated**: a comment line `# deprecated: use NAME` sits among the comment and decorator lines directly
  above the method's `def`. `NAME` is its replacement, another method of `Client`.
- **Paginated**: the method has a `page` parameter and a page-size parameter, named `per_page` or `page_size`.

## Rule 1: `account_ref` becomes `account_id`

The parameter `account_ref` is renamed `account_id`: same position, same default (or none), same keyword-only
status. Where v1 sends it as a query parameter or a body key, v2 sends the key `account_id` (same value) and no
`account_ref` key. Where it is a path placeholder, the path on the wire is unchanged; only the parameter name
changes.

## Rule 2: path placeholders are validated

A method first checks the value of each path placeholder, in the order the placeholders appear in the path. A
value is valid when it is a `str`, is not empty and contains no `/`. The first invalid value raises

    ValueError("<method>: invalid <parameter>: <repr(value)>")

for example `ValueError("get_order: invalid order_id: ''")` or `ValueError("get_order: invalid order_id: 7")`.
`<parameter>` is the parameter's v2 name (after rule 1). A method that raises does not call the transport.

## Rule 3: deprecated methods warn and delegate

A deprecated method keeps its name but does nothing of its own any more. When called it

1. emits exactly one warning, category `DeprecationWarning`, message exactly
   `<method>() is deprecated; use <replacement>() instead` (for example
   `fetch_order() is deprecated; use get_order() instead`), then
2. calls its replacement with the same arguments, passed by keyword, and returns what the replacement returns.

Its parameter list becomes the replacement's v2 parameter list (in v1 both already have the same parameters,
so rules 1 and 4 change them the same way). Validation, retries and body mapping happen in the replacement:
the deprecated method's own decorator flags and v1 path no longer matter, and a `ValueError` raised by the
replacement (naming the replacement) propagates unchanged after the warning.

## Rule 4: cursor pagination

In a paginated method, `page` is replaced by `cursor` (default `None`) and the page-size parameter by `limit`
(default: the page-size parameter's v1 default), each in the old parameter's position and with its
keyword-only status. On the wire the query has no `page`, `per_page` or `page_size` key any more; it has
`limit` (always) and `cursor` (only when `cursor` is not `None`). `limit` is valid when it is an `int` (not a
`bool`) from 1 to %(maxlimit)d inclusive; otherwise the method raises
`ValueError("<method>: invalid limit: <repr(limit)>")`.

## Rule 5: retries for idempotent methods

An idempotent method passes `retries=%(retries)d` to the transport. Every other method passes no `retries` (or 0).

## Rule 6: request body field mapping

The keys of the request body (`json=`, never the query) are mapped:

| v1 body key | v2 body key | v2 value (`v` = the v1 value) |
|---|---|---|
| `weight_lb` | `weight_kg` | `round(v * 0.45359237, 3)` |
| `length_in` | `length_cm` | `round(v * 2.54, 2)` |
| `temp_f` | `temp_c` | `round((v - 32) * 5 / 9, 1)` |
| `qty` | `quantity` | `v` |
| `desc` | `description` | `v` |

A key absent from the v1 body stays absent; other keys are unchanged. The Python parameter names do not change
(`create_shipment(..., weight_lb=12.0)` still takes pounds). A query parameter that shares a name with a table
key (for example the `desc` sort flag of some list methods) is not touched.

## Order of checks

Rule-2 checks first, then the rule-4 `limit` check, then the transport call. Arguments are not validated in
any other way.

## Exceptions

These methods differ from the rules above in exactly the way stated; every other rule still applies to them.

| Method | Rule | What differs |
|---|---|---|
%(table)s
"""


def migration_md(methods, exc):
    bym = by_name(methods)
    rows = []
    for n in exc["legacy"]:
        rows.append((1, n, "keeps `account_ref` (parameter name and wire key unchanged)"))
    for n, slash in sorted(exc["allow"].items()):
        rows.append((2, n, "`%s` may contain `/` (it must still be a non-empty `str`)" % slash))
    for n in exc["keep"]:
        rows.append((4, n, "keeps `page` and `%s` (no cursor pagination, no `limit` check)" % bym[n]["size"]))
    for n, r in sorted(exc["retries"].items()):
        rows.append((5, n, "passes `retries=%d`" % r))
    for n in exc["raw"]:
        rows.append((6, n, "request body sent unchanged (no field mapping)"))
    rows.sort()
    table = "\n".join("| `%s` | %d | %s |" % (n, r, t) for r, n, t in rows)
    return MIGRATION % {"count": len(methods), "areas": ", ".join(AREA_NAMES), "maxlimit": MAX_LIMIT,
                        "retries": RETRIES, "table": table}


# ------------------------------------------------------------------------------------------ visible tests


TEST_HEAD = '''"""Examples of the v2 behaviour of client.py (see MIGRATION.md).

Run from the workspace root: /usr/bin/python3 -m unittest discover -s tests -v
These cover a few methods only; the migration applies to every method of Client.
"""
import os
import sys
import unittest
import warnings

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from client import Client  # noqa: E402


class FakeTransport:
    def __init__(self):
        self.calls = []

    def request(self, verb, path, params=None, json=None, retries=0):
        self.calls.append((verb, path, dict(params or {}), dict(json or {}), retries))
        return {"seq": len(self.calls)}


def call(method, kwargs):
    t = FakeTransport()
    with warnings.catch_warnings(record=True) as w:
        warnings.simplefilter("always")
        result = getattr(Client(t), method)(**kwargs)
    return t.calls, result, [(x.category.__name__, str(x.message)) for x in w]


class ClientV2Test(unittest.TestCase):
    def assertCall(self, method, kwargs, expected):
        calls, result, _ = call(method, kwargs)
        self.assertEqual(calls, [expected])
        self.assertEqual(result, {"seq": 1})

    def assertRaisesMessage(self, method, kwargs, message):
        t = FakeTransport()
        with self.assertRaises(ValueError) as cm:
            getattr(Client(t), method)(**kwargs)
        self.assertEqual(str(cm.exception), message)
        self.assertEqual(t.calls, [])
'''


def v2_kwargs(m, exc, salt, tag, full=True):
    kw = {}
    for i, p in enumerate(m["params"]):
        n = v2_name(m, p, exc)
        if p["req"] or (full and p["role"] in ("query", "body", "qflag")):
            kw[n] = sample_value(p, salt + i, tag)
    return kw


def _fmt_call(c):
    return "(%r, %r, %r, %r, %r)" % (c[1], c[2], c[3], c[4], c[5])


def visible_tests(methods, exc):
    bym = by_name(methods)
    special = set(exc["legacy"]) | set(exc["allow"]) | set(exc["retries"]) | set(exc["keep"]) | set(exc["raw"])
    out = [TEST_HEAD]
    k = 0
    for area in AREA_NAMES[:3]:
        ms = [m for m in methods if m["area"] == area and not m["dep"] and m["name"] not in special]

        taken = set()

        def first(cond):
            for m in ms:
                if m["name"] not in taken and cond(m):
                    taken.add(m["name"])
                    return m
            raise AssertionError(area)
        chosen = [
            ("rename", first(lambda m: renamed(m, exc) and not m["ph"][1:])),
            ("placeholder", first(lambda m: len(m["ph"]) >= 1)),
            ("pagination", first(lambda m: paginated(m, exc))),
            ("retries", first(lambda m: m["idem"])),
            ("fields", first(lambda m: any(p["role"] == "body" and p["name"] in MAPPED for p in m["params"]))),
            ("unchanged", first(lambda m: not renamed(m, exc) and not m["size"] and not m["idem"]
                                and not any(p["name"] in MAPPED for p in m["params"]))),
        ]
        for what, m in chosen:
            k += 1
            salt = 900 + k * 17
            kw = v2_kwargs(m, exc, salt, "v")
            if what == "pagination":
                kw["cursor"] = "vc%d" % k
                kw["limit"] = 7 + k
            out.append("    def test_%s_%s(self):" % (what, m["name"]))
            out.append("        self.assertCall(%r, %r,\n                        %s)" % (m["name"], kw, _fmt_call(v2_call(m, kw, exc))))
            if what in ("placeholder", "pagination"):
                bad = dict(kw)
                if what == "placeholder":
                    x = m["ph"][-1]
                    n = "account_id" if (x == "account_ref" and m["name"] not in exc["legacy"]) else x
                    bad[n] = "" if k % 2 else 0
                else:
                    bad["limit"] = 0 if k % 2 else 500
                r = v2_call(m, bad, exc)
                out.append("        self.assertRaisesMessage(%r, %r,\n                                 %r)" % (m["name"], bad, r[2]))
            out.append("")
        d = [m for m in methods if m["area"] == area and m["dep"]][0]
        rep = bym[d["dep"]]
        kw = v2_kwargs(rep, exc, 950 + k, "v")
        out.append("    def test_deprecated_%s(self):" % d["name"])
        out.append("        calls, result, warned = call(%r, %r)" % (d["name"], kw))
        out.append("        self.assertEqual(warned, [(\"DeprecationWarning\", %r)])" % dep_message(d))
        out.append("        self.assertEqual(calls, [%s])" % _fmt_call(v2_call(rep, kw, exc)))
        out.append("        self.assertEqual(result, {\"seq\": 1})")
        out.append("")
    for n in (exc["legacy"][0], sorted(exc["allow"])[0]):
        m = bym[n]
        kw = v2_kwargs(m, exc, 990, "v")
        if n in exc["allow"]:
            kw[exc["allow"][n]] = "v/nested/key"
        out.append("    def test_exception_%s(self):" % n)
        out.append("        self.assertCall(%r, %r,\n                        %s)" % (n, kw, _fmt_call(v2_call(m, kw, exc))))
        out.append("")
    out.append("")
    out.append('if __name__ == "__main__":')
    out.append("    unittest.main()")
    return "\n".join(out) + "\n"


def write_workspace(ws):
    methods, exc = build()
    with open(os.path.join(ws, "client.py"), "w") as f:
        f.write(render_client(methods))
    with open(os.path.join(ws, "MIGRATION.md"), "w") as f:
        f.write(migration_md(methods, exc))
    os.makedirs(os.path.join(ws, "tests"), exist_ok=True)
    with open(os.path.join(ws, "tests", "test_client.py"), "w") as f:
        f.write(visible_tests(methods, exc))


if __name__ == "__main__":
    write_workspace(sys.argv[1])
