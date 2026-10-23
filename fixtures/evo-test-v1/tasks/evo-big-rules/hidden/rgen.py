"""evo-big-rules: generator of the initial workspace, and the behaviour model the check uses.

As a script (`rgen.py WS`, run by workspace.gen) it writes WS/rules.py (the generated v1 rule table),
WS/MIGRATION.md (the v1 -> v2 rules, the code table, the retirements and the exceptions) and
WS/tests/test_rules.py (visible examples). Imported, build() returns the catalogue and v2() gives the migrated
behaviour. Deterministic: one seeded random.Random, no clock, no environment. Python 3.9, stdlib only.
"""
import datetime
import os
import random
import re
import sys

SEED = 4127
TODAY = "2026-01-15"
CODES = [
    ("E_MISSING", "REQ.MISSING"), ("E_STRICT_MISSING", "REQ.STRICT"), ("E_RANGE", "NUM.RANGE"),
    ("E_TOO_LONG", "TXT.LENGTH"), ("E_FORMAT", "TXT.FORMAT"), ("E_ID", "ID.FORMAT"), ("E_EMAIL", "TXT.EMAIL"),
    ("E_CURRENCY", "TXT.CURRENCY"), ("E_UNKNOWN", "ENUM.UNKNOWN"), ("E_DATE", "DATE.INVALID"),
    ("E_DATE_FUTURE", "DATE.FUTURE"), ("E_DATE_STALE", "DATE.STALE"), ("E_ORDER", "CROSS.ORDER"),
    ("E_BLANK", "TXT.BLANK"), ("E_TOO_SHORT", "TXT.SHORT"), ("E_TYPE", "NUM.TYPE"),
    ("E_PRECISION", "NUM.PRECISION"), ("E_CASE", "TXT.CASE"),
]
CODE_MAP = dict(CODES)
# codes whose v2 name depends on the rule's category: (v1 code, category) -> v2 code; others use CODE_MAP
CODE_BY_CAT = {("E_MISSING", "identity"): "ID.MISSING", ("E_RANGE", "money"): "MONEY.RANGE",
               ("E_PRECISION", "money"): "MONEY.CENTS", ("E_FORMAT", "identity"): "ID.FORMAT"}


def map_code(code, category):
    return CODE_BY_CAT.get((code, category)) or CODE_MAP[code]
CATEGORIES = ["money", "identity", "dates", "format", "style", "consistency"]
PATTERNS = {
    "id": r"[A-Z]{3}-\d{4}", "currency": r"[A-Z]{3}", "email": r"[^@\s]+@[^@\s]+\.[a-z]{2,}",
    "country": r"[A-Z]{2}", "po": r"PO-\d{6}", "barcode": r"\d{13}", "tracking": r"[A-Z]{2}\d{9}[A-Z]{2}",
    "postcode": r"\d{5}", "vat": r"[A-Z]{2}\d{8,10}", "phone": r"\+\d{8,15}", "tag": r"AS-\d{5}",
    "ref": r"[A-Z0-9]{8,16}", "sku": r"[A-Z]{2}-\d{4}",
}
PAT_VALUE = {"currency": "EUR", "email": "ana@example.org", "country": "DE", "po": "PO-123456",
             "barcode": "4006381333931", "tracking": "RA123456789DE", "postcode": "10115", "vat": "DE123456789",
             "phone": "+4930123456", "tag": "AS-00042", "ref": "AB12CD34", "sku": "KX-2041"}
PAT_CODE = {"id": "E_ID", "currency": "E_CURRENCY", "email": "E_EMAIL"}
ALT_CTX = [{"today": TODAY, "strict": False}, {"today": "2025-03-10", "strict": False},
           {"today": "2027-08-20", "strict": True}, {"today": "2026-01-15", "strict": True}]

# section, singular, id prefix, fields {name: spec}, order pairs, strict-only fields
# spec: ("id",) ("str", maxlens) ("num", lo, hi, base, money) ("int", lo, hi, base) ("date", base, past)
#       ("enum", values) ("pat", kind)
SECTIONS = [
    ("invoices", "invoice", "INV", {
        "invoice_id": ("id",), "customer_id": ("id",), "amount": ("num", 0.01, 1000000, 1250.5, True),
        "currency": ("pat", "currency"), "issue_date": ("date", "2025-11-02", True),
        "due_date": ("date", "2025-12-02", True), "tax_rate": ("num", 0.0, 0.5, 0.19, True),
        "memo": ("str", [40, 80]), "status": ("enum", ["draft", "open", "paid", "void"]), "po_number": ("pat", "po"),
    }, [("issue_date", "due_date")], ["memo", "po_number"]),
    ("payments", "payment", "PAY", {
        "payment_id": ("id",), "invoice_id": ("id",), "amount": ("num", 0.01, 500000, 980.0, True),
        "currency": ("pat", "currency"), "paid_on": ("date", "2025-12-20", True), "fee": ("num", 0, 250, 2.5, True),
        "method": ("enum", ["card", "wire", "ach", "cash"]), "reference": ("pat", "ref"),
        "payer_email": ("pat", "email"),
    }, [("fee", "amount")], ["reference", "payer_email"]),
    ("customers", "customer", "CUS", {
        "customer_id": ("id",), "name": ("str", [60, 120]), "email": ("pat", "email"), "country": ("pat", "country"),
        "vat_id": ("pat", "vat"), "credit_limit": ("num", 0, 1000000, 5000, True),
        "segment": ("enum", ["smb", "mid", "enterprise"]), "signup_date": ("date", "2025-09-14", True),
        "phone": ("pat", "phone"), "city": ("str", [40, 80]), "discount": ("num", 0.0, 0.5, 0.05, True),
    }, [], ["vat_id", "phone"]),
    ("products", "product", "PRD", {
        "sku": ("pat", "sku"), "title": ("str", [60, 100]), "price": ("num", 0.01, 100000, 19.99, True),
        "weight_kg": ("num", 0.01, 1000, 1.2, False), "category": ("enum", ["tools", "garden", "kitchen", "toys"]),
        "barcode": ("pat", "barcode"), "launch_date": ("date", "2025-10-01", True), "stock": ("int", 0, 100000, 40),
        "product_id": ("id",),
    }, [], ["barcode", "category"]),
    ("shipments", "shipment", "SHP", {
        "shipment_id": ("id",), "order_id": ("id",), "carrier": ("enum", ["dhl", "ups", "gls", "post"]),
        "ship_date": ("date", "2026-01-05", True), "delivery_date": ("date", "2026-01-09", True),
        "weight_kg": ("num", 0.01, 2000, 3.4, False), "tracking": ("pat", "tracking"),
        "postcode": ("pat", "postcode"), "parcels": ("int", 1, 50, 2),
    }, [("ship_date", "delivery_date")], ["tracking", "postcode"]),
    ("employees", "employee", "EMP", {
        "employee_id": ("id",), "email": ("pat", "email"), "hire_date": ("date", "2025-08-01", True),
        "birth_date": ("date", "1990-04-12", False), "salary": ("num", 1000, 500000, 52000, True),
        "department": ("enum", ["ops", "sales", "finance", "it"]), "manager_id": ("id",),
        "title": ("str", [50, 90]), "country": ("pat", "country"),
    }, [("birth_date", "hire_date")], ["manager_id", "title"]),
    ("contracts", "contract", "CON", {
        "contract_id": ("id",), "customer_id": ("id",), "start_date": ("date", "2025-12-01", True),
        "end_date": ("date", "2026-11-30", False), "value": ("num", 1, 10000000, 12000, True),
        "renewal": ("enum", ["auto", "manual", "none"]), "signed_on": ("date", "2025-11-20", True),
        "notice_days": ("int", 0, 365, 30), "currency": ("pat", "currency"),
    }, [("start_date", "end_date"), ("signed_on", "start_date")], ["renewal", "notice_days"]),
    ("assets", "asset", "AST", {
        "asset_id": ("id",), "tag": ("pat", "tag"), "purchase_date": ("date", "2025-10-10", True),
        "cost": ("num", 1, 2000000, 899.0, True), "location": ("enum", ["hq", "plant", "depot", "remote"]),
        "owner_id": ("id",), "warranty_until": ("date", "2027-10-10", False), "serial": ("str", [32, 64]),
        "units": ("int", 1, 1000, 1),
    }, [("purchase_date", "warranty_until")], ["location", "serial"]),
    ("orders", "order", "ORD", {
        "order_id": ("id",), "customer_id": ("id",), "order_date": ("date", "2025-12-28", True),
        "ship_by": ("date", "2026-01-04", True), "total": ("num", 0.01, 250000, 149.9, True),
        "currency": ("pat", "currency"), "channel": ("enum", ["web", "store", "phone", "partner"]),
        "items": ("int", 1, 500, 3), "coupon": ("pat", "ref"), "note": ("str", [80, 200]),
    }, [("order_date", "ship_by")], ["coupon", "note"]),
    ("tickets", "ticket", "TCK", {
        "ticket_id": ("id",), "opened_on": ("date", "2026-01-02", True), "closed_on": ("date", "2026-01-08", True),
        "priority": ("enum", ["low", "normal", "high", "urgent"]), "subject": ("str", [72, 120]),
        "reporter_email": ("pat", "email"), "sla_hours": ("int", 1, 720, 48), "product_sku": ("pat", "sku"),
        "assignee_id": ("id",),
    }, [("opened_on", "closed_on")], ["assignee_id", "product_sku"]),
]
SECTION_NAMES = [s[0] for s in SECTIONS]
COMPOSED_SUFFIX = ["complete", "consistent", "ready", "basic", "core", "sane", "valid_for_export"]


# ------------------------------------------------------------------------------------------ helpers (model)


def is_number(v):
    return isinstance(v, (int, float)) and not isinstance(v, bool)


def is_date(v):
    if not isinstance(v, str) or not re.fullmatch(r"\d{4}-\d{2}-\d{2}", v):
        return False
    try:
        datetime.date.fromisoformat(v)
    except ValueError:
        return False
    return True


def days_between(a, b):
    return (datetime.date.fromisoformat(a) - datetime.date.fromisoformat(b)).days


def fmt_num(x):
    return str(x)


# ------------------------------------------------------------------------------------------ catalogue


def _cands(rng, sec):
    name, single, prefix, fields, orders, strict = sec
    out = []

    def add(tpl, field, **kw):
        d = {"tpl": tpl, "field": field}
        d.update(kw)
        out.append(d)
    for f, spec in fields.items():
        t = spec[0]
        if t == "id":
            add("required", f, cat="identity")
            add("pattern", f, kind="id", cat="identity")
        elif t == "str":
            if f not in strict:
                add("required", f, cat="consistency")
            add("max_len", f, n=rng.choice(spec[1]), cat="style")
            add("not_blank", f, cat=rng.choice(["style", "consistency"]))
            add("min_len", f, n=rng.choice([2, 3, 4]), cat="style")
        elif t == "num":
            money = spec[4]
            add("positive", f, cat="money" if money else "consistency")
            add("range", f, lo=spec[1], hi=spec[2], cat="money" if money else "consistency")
            if money:
                add("precision", f, cat="money")
        elif t == "int":
            add("range", f, lo=spec[1], hi=spec[2], cat="consistency")
            add("integer", f, cat="consistency")
        elif t == "date":
            add("date", f, cat="dates")
            if spec[2]:
                add("not_future", f, cat="dates")
                if rng.random() < 0.7:
                    add("max_age", f, days=rng.choice([365, 540, 730]), cat="dates")
        elif t == "enum":
            add("one_of", f, values=spec[1], cat=rng.choice(["consistency", "style"]))
            if rng.random() < 0.5 and f not in strict:
                add("required", f, cat="consistency")
        else:
            kind = spec[1]
            add("pattern", f, kind=kind, cat="identity" if kind in ("email", "vat", "phone") else "format")
            if kind == "email":
                add("lowercase", f, cat="style")
    for a, b in orders:
        add("order", a, other=b, cat=rng.choice(["consistency", "dates"]))
    for f in strict:
        add("strict_required", f, cat=rng.choice(["style", "consistency"]))
    for c in out:
        if rng.random() < 0.15:
            c["cat"] = rng.choice(CATEGORIES)
    return out


def _fn_name(single, c, used):
    tpl = c["tpl"]
    suffix = {"required": "required", "pattern": "format", "max_len": "length", "positive": "positive",
              "range": "in_range", "date": "valid_date", "not_future": "not_future", "max_age": "recent",
              "one_of": "known", "order": "before_" + c.get("other", ""), "strict_required": "strict_required",
              "not_blank": "not_blank", "min_len": "min_length", "integer": "integer", "precision": "cents",
              "lowercase": "lowercase"}[tpl]
    base = "%s_%s_%s" % (single, c["field"], suffix)
    name, k = base, 2
    while name in used:
        name = "%s_%d" % (base, k)
        k += 1
    used.add(name)
    return name


def build():
    """The catalogue: {rules: [rule dicts in file order], retired: {old: new}, merged: {away: target},
    keep_code: [ids], pinned: {id: severity}, strict_sub: [ids]}"""
    rng = random.Random(SEED)
    rules = []
    used = set()
    n = 0
    for sec in SECTIONS:
        sname, single = sec[0], sec[1]
        cands = _cands(rng, sec)
        strict_c = [c for c in cands if c["tpl"] == "strict_required"]
        ctx_c = [c for c in cands if c["tpl"] in ("not_future", "max_age")]
        other = [c for c in cands if c["tpl"] not in ("not_future", "max_age", "strict_required")]
        rng.shuffle(other)
        rng.shuffle(ctx_c)
        pick = strict_c + ctx_c[:3]
        pick += other[:20 - len(pick)]
        assert len(pick) == 20, (sname, len(cands))
        rng.shuffle(pick)
        plain = []
        for c in pick:
            n += 1
            r = dict(c)
            r.update({"id": "R%04d" % n, "section": sname, "fn": _fn_name(single, c, used),
                      "sev": "error" if rng.random() < 0.6 else "warning", "subs": None,
                      "valmsg": c["tpl"] in ("positive", "range", "one_of") and rng.random() < 0.5})
            r["code"] = {"required": "E_MISSING", "pattern": PAT_CODE.get(c.get("kind"), "E_FORMAT"),
                         "max_len": "E_TOO_LONG", "positive": "E_RANGE", "range": "E_RANGE", "date": "E_DATE",
                         "not_future": "E_DATE_FUTURE", "max_age": "E_DATE_STALE", "one_of": "E_UNKNOWN",
                         "order": "E_ORDER", "strict_required": "E_STRICT_MISSING", "not_blank": "E_BLANK",
                         "min_len": "E_TOO_SHORT", "integer": "E_TYPE", "precision": "E_PRECISION",
                         "lowercase": "E_CASE"}[c["tpl"]]
            r["msg"] = _message(r)
            plain.append(r)
        rules += plain
        sec_rules = list(plain)
        for suffix in rng.sample(COMPOSED_SUFFIX, 3):
            n += 1
            subs = rng.sample(sec_rules, rng.choice([2, 3, 3, 4]))
            name = "%s_%s" % (single, suffix)
            assert name not in used
            used.add(name)
            rules.append({"id": "R%04d" % n, "section": sname, "fn": name, "tpl": "composed", "field": None,
                          "subs": [s["id"] for s in subs], "cat": "consistency", "sev": None, "code": None,
                          "msg": None, "valmsg": False, "style": None})
    byid = dict((r["id"], r) for r in rules)
    # retirements and merges: plain, non-context rules of a section, never sub-rules' only reference
    cat = {"rules": rules, "retired": {}, "merged": {}, "keep_code": [], "pinned": {}, "strict_sub": []}
    taken = set()

    def plain_of(s, ctx_ok=False):
        return [r for r in rules if r["section"] == s and r["tpl"] != "composed" and r["id"] not in taken
                and (ctx_ok or r["tpl"] not in ("not_future", "max_age", "strict_required"))]
    def pair(s):
        c = plain_of(s)
        same = [(x, y) for x in c for y in c if x["id"] != y["id"] and x["field"] == y["field"]]
        return rng.choice(same) if same else tuple(rng.sample(c, 2))
    for s in SECTION_NAMES[:6]:
        old, new = pair(s)
        cat["retired"][old["id"]] = new["id"]
        taken.update([old["id"], new["id"]])
    for s in SECTION_NAMES[6:]:
        b, a = pair(s)
        cat["merged"][b["id"]] = a["id"]
        taken.update([a["id"], b["id"]])
    for s in [SECTION_NAMES[i] for i in (0, 2, 4, 6, 8)]:
        r = rng.choice(plain_of(s, True))
        cat["keep_code"].append(r["id"])
        taken.add(r["id"])
    for s, sev in zip([SECTION_NAMES[i] for i in (1, 3, 5, 7, 9)], ["blocker", "warning", "info", "error", "blocker"]):
        r = rng.choice(plain_of(s, True))
        cat["pinned"][r["id"]] = sev
        taken.add(r["id"])
    # make sure some composed rules reference a retired rule, and two composed rules get strict sub-rules
    comps = [r for r in rules if r["tpl"] == "composed"]
    for k, (old, new) in enumerate(sorted(cat["retired"].items())[:3]):
        c = [r for r in comps if r["section"] == byid[old]["section"]][k % 3]
        if old not in c["subs"]:
            c["subs"][rng.randrange(len(c["subs"]))] = old
    for s in ("customers", "contracts"):
        c = [r for r in comps if r["section"] == s][1]
        strict = [r for r in rules if r["section"] == s and r["tpl"] == "strict_required"
                  and r["id"] not in cat["retired"]]
        if not any(byid[x]["tpl"] == "strict_required" for x in c["subs"]):
            c["subs"].append(strict[0]["id"])
        cat["strict_sub"].append(c["id"])
    for r in comps:
        r["subs"] = list(dict.fromkeys(r["subs"]))
    cat["keep_code"].sort()
    cat["strict_sub"].sort()
    return cat


def _message(r):
    f, t = r["field"], r["tpl"]
    if t == "required":
        return "%s is required" % f
    if t == "strict_required":
        return "%s is required in strict mode" % f
    if t == "pattern":
        return "%s has an invalid format" % f
    if t == "max_len":
        return "%s is longer than %d characters" % (f, r["n"])
    if t == "positive":
        return "%s must be positive" % f
    if t == "range":
        return "%s must be between %s and %s" % (f, fmt_num(r["lo"]), fmt_num(r["hi"]))
    if t == "date":
        return "%s is not a valid date" % f
    if t == "not_future":
        return "%s is in the future" % f
    if t == "max_age":
        return "%s is older than %d days" % (f, r["days"])
    if t == "one_of":
        return "%s must be one of %s" % (f, ", ".join(r["values"]))
    if t == "order":
        return "%s must not be after %s" % (f, r["other"])
    if t == "not_blank":
        return "%s must not be blank" % f
    if t == "min_len":
        return "%s is shorter than %d characters" % (f, r["n"])
    if t == "integer":
        return "%s must be an integer" % f
    if t == "precision":
        return "%s has more than 2 decimals" % f
    if t == "lowercase":
        return "%s must be lower case" % f
    raise AssertionError(t)


# ------------------------------------------------------------------------------------------ model


def by_id(cat):
    return dict((r["id"], r) for r in cat["rules"])


def own_v1(r, rec, ctx):
    """The rule's own v1 check (a (code, severity, message) tuple or None); ctx supplies today/strict."""
    t, f = r["tpl"], r["field"]
    v = rec.get(f)
    bad = False
    if t == "required":
        bad = v is None or v == ""
    elif t == "strict_required":
        bad = bool(ctx["strict"]) and (v is None or v == "")
    elif t == "pattern":
        bad = v is not None and not (isinstance(v, str) and re.fullmatch(PATTERNS[r["kind"]], v))
    elif t == "max_len":
        bad = v is not None and len(str(v)) > r["n"]
    elif t == "positive":
        bad = v is not None and (not is_number(v) or v <= 0)
    elif t == "range":
        bad = v is not None and (not is_number(v) or not r["lo"] <= v <= r["hi"])
    elif t == "date":
        bad = v is not None and not is_date(v)
    elif t == "not_future":
        bad = is_date(v) and v > ctx["today"]
    elif t == "max_age":
        bad = is_date(v) and days_between(ctx["today"], v) > r["days"]
    elif t == "one_of":
        bad = v is not None and v not in r["values"]
    elif t == "not_blank":
        bad = isinstance(v, str) and v.strip() == ""
    elif t == "min_len":
        bad = v is not None and len(str(v)) < r["n"]
    elif t == "integer":
        bad = v is not None and not (isinstance(v, int) and not isinstance(v, bool))
    elif t == "precision":
        bad = is_number(v) and round(v, 2) != v
    elif t == "lowercase":
        bad = isinstance(v, str) and v != v.lower()
    elif t == "order":
        w = rec.get(r["other"])
        if is_date(v) and is_date(w):
            bad = v > w
        elif is_number(v) and is_number(w):
            bad = v > w
    if not bad:
        return None
    msg = r["msg"]
    if r["valmsg"]:
        msg = "%s (got %r)" % (msg, v)
    return (r["code"], r["sev"], msg)


def split_sev(sev, category):
    if sev == "error" and category in ("money", "identity"):
        return "blocker"
    if sev == "warning" and category == "style":
        return "info"
    return sev


def finish(cat, rid, category, f):
    if f is None:
        return None
    code = f[0] if rid in cat["keep_code"] else map_code(f[0], category)
    sev = cat["pinned"].get(rid) or split_sev(f[1], category)
    return (rid, code, sev, f[2])


def v2(cat, rid, rec, ctx, byid=None):
    """The v2 result of calling rule rid's function with (rec, ctx)."""
    byid = byid or by_id(cat)
    if rid in cat["retired"]:
        return v2(cat, cat["retired"][rid], rec, ctx, byid)
    if rid in cat["merged"]:
        return v2(cat, cat["merged"][rid], rec, ctx, byid)
    r = byid[rid]
    if r["tpl"] == "composed":
        sub_ctx = dict(ctx, strict=True) if rid in cat["strict_sub"] else ctx
        for s in r["subs"]:
            f = v2(cat, s, rec, sub_ctx, byid)
            if f is not None:
                return f
        return None
    f = finish(cat, rid, r["cat"], own_v1(r, rec, ctx))
    if f is None:
        for b, a in cat["merged"].items():
            if a == rid:
                f = finish(cat, rid, r["cat"], own_v1(byid[b], rec, ctx))
    return f


def v1(cat, rid, rec, byid=None):
    """The v1 result of calling rule rid's function with (rec,) (module TODAY, STRICT = False)."""
    byid = byid or by_id(cat)
    r = byid[rid]
    ctx = {"today": TODAY, "strict": False}
    if r["tpl"] == "composed":
        for s in r["subs"]:
            f = v1(cat, s, rec, byid)
            if f is not None:
                return f
        return None
    return own_v1(r, rec, ctx)


def live_ids(cat):
    return [r["id"] for r in cat["rules"] if r["id"] not in cat["retired"] and r["id"] not in cat["merged"]]


# ------------------------------------------------------------------------------------------ probe records


def base_record(sec):
    name, single, prefix, fields, orders, strict = sec
    rec = {}
    for i, (f, spec) in enumerate(sorted(fields.items())):
        t = spec[0]
        if t == "id":
            rec[f] = "%s-%04d" % (prefix if f.startswith(single) else f[:3].upper(), 17 + i * 7)
        elif t == "str":
            rec[f] = "%s %s" % (single.capitalize(), f.replace("_", " "))
        elif t == "num":
            rec[f] = spec[3]
        elif t == "int":
            rec[f] = spec[3]
        elif t == "date":
            rec[f] = spec[1]
        elif t == "enum":
            rec[f] = spec[1][0]
        else:
            rec[f] = PAT_VALUE[spec[1]]
    return rec


def _shift(date, days):
    return (datetime.date.fromisoformat(date) + datetime.timedelta(days=days)).isoformat()


def bad_values(r, salt):
    """Values of r's field that make r's own check fail (or pass, for cross-field and context rules)."""
    t = r["tpl"]
    if t == "required":
        return [None, ""]
    if t == "strict_required":
        return [None, ""]
    if t == "pattern":
        return [["bad value", "x" + PAT_VALUE.get(r["kind"], "AAA-0001")[1:].lower(), 4711, ""][salt % 4],
                ["  ", "BAD", 12.5, "abc-1234"][(salt + 1) % 4]]
    if t == "max_len":
        return ["y" * (r["n"] + 1 + salt % 3), 10 ** (r["n"] + 1)]
    if t == "positive":
        return [[0, -3, -0.5][salt % 3], ["12", True, None][salt % 2]]
    if t == "range":
        return [r["lo"] - [1, 0.5][salt % 2], r["hi"] + [1, 10][salt % 2], "%s" % r["hi"]]
    if t == "date":
        return [["2025-13-01", "2025-02-30", "2025/01/05"][salt % 3], [20250105, "05.01.2025", "2025-1-5"][salt % 3]]
    if t == "not_future":
        return [_shift(TODAY, 3 + salt % 40), "2030-06-01"]
    if t == "max_age":
        return [_shift(TODAY, -(r["days"] + 1 + salt % 20)), "2019-01-31"]
    if t == "one_of":
        return [["other", r["values"][0].upper(), ""][salt % 3], 3]
    if t == "order":
        return []
    if t == "not_blank":
        return [["   ", "\t", ""][salt % 3], " \n "]
    if t == "min_len":
        return ["x" * (r["n"] - 1), ""]
    if t == "integer":
        return [[2.5, "3", True][salt % 3], 7.0]
    if t == "precision":
        return [[10.123, 0.005, 99.999][salt % 3], 1.0001]
    if t == "lowercase":
        return [["Ana@Example.org", "BOB@EXAMPLE.ORG", "x@Y.com"][salt % 3], "Mixed.Case@example.org"]
    raise AssertionError(t)


def probe_records(cat, r, sec, salt):
    """Records for rule r: the base record, records breaking r's own check, edge records that pass."""
    base = base_record(sec)
    recs = [dict(base)]
    t = r["tpl"]
    if t == "composed":
        byid = by_id(cat)
        subs = [byid[cat["retired"].get(s, s)] for s in r["subs"]]
        for k, s in enumerate(subs):
            for rec in probe_records(cat, s, sec, salt + k)[1:3]:
                recs.append(rec)
        two = dict(base)
        for k, s in enumerate(subs[-2:]):
            more = probe_records(cat, s, sec, salt + 7 + k)
            if len(more) > 1:
                for key in base:
                    if key not in more[1]:
                        two.pop(key, None)
                    elif more[1][key] != base[key]:
                        two[key] = more[1][key]
        recs.append(two)
        return recs
    f = r["field"]
    if t == "order":
        a, b = base[f], base[r["other"]]
        x = dict(base)
        x[f], x[r["other"]] = b, a
        recs.append(x)
        y = dict(base)
        y[r["other"]] = None
        recs.append(y)
        z = dict(base)
        z[f] = z[r["other"]]
        recs.append(z)
        return recs
    for v in bad_values(r, salt):
        x = dict(base)
        if v is None:
            del x[f]
        else:
            x[f] = v
        recs.append(x)
    if t in ("positive", "range", "pattern", "one_of", "date", "max_len", "min_len", "integer", "precision",
             "lowercase", "not_blank"):
        x = dict(base)
        del x[f]
        recs.append(x)
    return recs


# ------------------------------------------------------------------------------------------ render v1


HEADER = '''"""Ledger record validation rules (v1).

Generated from the rule catalogue: one function per rule, registered in RULES at the end of the file.

A rule takes a record (a dict) and returns None when the record passes, or a finding: a tuple
(code, severity, message) where severity is "error" or "warning". Rules that compare dates with today use
TODAY; a few optional checks only run when STRICT is set. A composed rule runs other rules and returns the
first finding. RULES maps each rule id to (function, category).

MIGRATION.md describes the move to the v2 rule interface.
"""
import datetime
import re

TODAY = "%s"
STRICT = False

ERROR = "error"
WARNING = "warning"

E_MISSING = "E_MISSING"
E_RANGE = "E_RANGE"
E_FORMAT = "E_FORMAT"
E_DATE = "E_DATE"


def _is_number(value):
    return isinstance(value, (int, float)) and not isinstance(value, bool)


def _is_date(value):
    if not isinstance(value, str) or not re.fullmatch(r"\\d{4}-\\d{2}-\\d{2}", value):
        return False
    try:
        datetime.date.fromisoformat(value)
    except ValueError:
        return False
    return True


def _days_since(day, today):
    return (datetime.date.fromisoformat(today) - datetime.date.fromisoformat(day)).days


def _fail(code, severity, message):
    return (code, severity, message)


def _require(record, field, severity):
    """Shared check: the field must be present and not empty."""
    value = record.get(field)
    if value is None or value == "":
        return (E_MISSING, severity, "%%s is required" %% field)
    return None


def _match(record, field, pattern, code, severity):
    """Shared check: when present, the field must be a string matching pattern."""
    value = record.get(field)
    if value is not None and not (isinstance(value, str) and re.fullmatch(pattern, value)):
        return (code, severity, "%%s has an invalid format" %% field)
    return None


def _in_future(day):
    return _is_date(day) and day > TODAY


def _too_old(day, days):
    return _is_date(day) and _days_since(day, TODAY) > days


def _strict():
    return STRICT


def _first(record, *rules):
    """Shared runner for composed rules: the first finding of rules, in order."""
    for rule in rules:
        finding = rule(record)
        if finding is not None:
            return finding
    return None
''' % TODAY

CODE_CONSTS = ("E_MISSING", "E_RANGE", "E_FORMAT", "E_DATE")
SUMMARY = {
    "required": "{f} must be present.", "strict_required": "{f} must be present in strict mode.",
    "pattern": "{f} must match the {k} format.", "max_len": "{f} must not be longer than {n} characters.",
    "positive": "{f} must be a positive number.", "range": "{f} must lie between {lo} and {hi}.",
    "date": "{f} must be an ISO date (YYYY-MM-DD).", "not_future": "{f} must not lie after today.",
    "max_age": "{f} must not be older than {d} days.", "one_of": "{f} must be one of the known values.",
    "order": "{f} must not come after {o}.", "not_blank": "{f} must not be blank when given.",
    "min_len": "{f} must be at least {n} characters long.", "integer": "{f} must be a whole number.",
    "precision": "{f} must not carry fractions of a cent.", "lowercase": "{f} must be written in lower case.",
}


def _finding_expr(r, rng, value_var):
    code = r["code"]
    code_e = code if (code in CODE_CONSTS and rng.random() < 0.5) else '"%s"' % code
    sev_e = (r["sev"].upper() if rng.random() < 0.4 else '"%s"' % r["sev"])
    msg_e = '"%s"' % r["msg"]
    if r["valmsg"]:
        msg_e = '"%s (got %%r)" %% (%s,)' % (r["msg"], value_var)
    style = rng.choice(["tuple", "tuple", "bare", "fail"])
    if style == "fail":
        return "_fail(%s, %s, %s)" % (code_e, sev_e, msg_e)
    if style == "bare":
        return "%s, %s, %s" % (code_e, sev_e, msg_e)
    return "(%s, %s, %s)" % (code_e, sev_e, msg_e)


def _cond(r, var):
    t = r["tpl"]
    if t == "required":
        return "%s is None or %s == \"\"" % (var, var), "%s in (None, \"\")" % var
    if t == "strict_required":
        return "STRICT and (%s is None or %s == \"\")" % (var, var), "STRICT and %s in (None, \"\")" % var
    if t == "pattern":
        p = 'not (isinstance(%s, str) and re.fullmatch(r"%s", %s))' % (var, PATTERNS[r["kind"]], var)
        return "%s is not None and %s" % (var, p), None
    if t == "max_len":
        return "%s is not None and len(str(%s)) > %d" % (var, var, r["n"]), None
    if t == "positive":
        return "%s is not None and (not _is_number(%s) or %s <= 0)" % (var, var, var), None
    if t == "range":
        return "%s is not None and (not _is_number(%s) or not %s <= %s <= %s)" % (
            var, var, fmt_num(r["lo"]), var, fmt_num(r["hi"])), None
    if t == "date":
        return "%s is not None and not _is_date(%s)" % (var, var), None
    if t == "not_future":
        return "_is_date(%s) and %s > TODAY" % (var, var), None
    if t == "max_age":
        return "_is_date(%s) and _days_since(%s, TODAY) > %d" % (var, var, r["days"]), None
    if t == "one_of":
        return "%s is not None and %s not in (%s)" % (var, var, ", ".join('"%s"' % x for x in r["values"])), None
    if t == "not_blank":
        return "isinstance(%s, str) and %s.strip() == \"\"" % (var, var), None
    if t == "min_len":
        return "%s is not None and len(str(%s)) < %d" % (var, var, r["n"]), None
    if t == "integer":
        return "%s is not None and (not isinstance(%s, int) or isinstance(%s, bool))" % (var, var, var), None
    if t == "precision":
        return "_is_number(%s) and round(%s, 2) != %s" % (var, var, var), None
    if t == "lowercase":
        return "isinstance(%s, str) and %s != %s.lower()" % (var, var, var), None
    raise AssertionError(t)


def _doc(r, rng, sec_single, byid):
    if r["tpl"] == "composed":
        head = "%s %s record passes %s." % (r["id"], sec_single.capitalize(), ", ".join(r["subs"]))
        lines = ['"""%s' % head, "", "Runs these rules in this order and returns the first finding:", ""]
        lines += ["    %s  %s" % (x, byid[x]["fn"]) for x in r["subs"]]
        lines += ["", "Returns None when every rule passes.", '"""']
        return lines if rng.random() < 0.85 else ['"""%s"""' % head]
    s = SUMMARY[r["tpl"]].format(f=r["field"], k=r.get("kind"), n=r.get("n"), lo=r.get("lo"), hi=r.get("hi"),
                                 d=r.get("days"), o=r.get("other"))
    head = "%s %s" % (r["id"], s) if rng.random() > 0.12 else s
    x = rng.random()
    if x < 0.08:
        return ['"""%s"""' % head]
    out = ['"""%s' % head, ""]
    out.append("Applies to: %s records." % sec_single)
    out.append("Field: %s%s" % (r["field"], (", " + r["other"]) if r["tpl"] == "order" else ""))
    out.append("Since: catalogue v1.%d" % (1 + int(r["id"][1:]) % 9))
    if x < 0.9:
        bad = bad_values(r, 0)
        out += ["", "Examples:", "    passes: a record whose %s satisfies the condition" % r["field"],
                "    passes: a record without %s, unless the rule requires it" % r["field"],
                "    fails: a record that breaks the condition above"]
        if bad and bad[0] is not None and r["tpl"] not in ("strict_required", "not_future", "max_age"):
            out.append("    fails: %s = %r" % (r["field"], bad[0]))
    same = [q["id"] for q in byid.values() if q["section"] == r["section"] and q.get("field") == r["field"]
            and q["id"] != r["id"]]
    if same and rng.random() < 0.7:
        out += ["", "See also: %s." % ", ".join(same)]
    if x > 0.35:
        out += ["", "Notes:", "    The message text is part of the report format; keep it stable."]
    out.append('"""')
    return out


def _sev_expr(r, rng):
    return r["sev"].upper() if rng.random() < 0.4 else '"%s"' % r["sev"]


def _plain_body(r, rng):
    f = r["field"]
    t = r["tpl"]
    out = []
    if t == "required" and rng.random() < 0.4:
        return ['return _require(record, "%s", %s)' % (f, _sev_expr(r, rng))]
    if t == "pattern" and rng.random() < 0.4:
        code = r["code"] if (r["code"] in CODE_CONSTS and rng.random() < 0.5) else '"%s"' % r["code"]
        return ['return _match(record, "%s", r"%s", %s, %s)' % (f, PATTERNS[r["kind"]], code, _sev_expr(r, rng))]
    if t == "order":
        out.append('first, second = record.get("%s"), record.get("%s")' % (f, r["other"]))
        kinds = rng.choice(["both", "both", "split"])
        cond_d = "_is_date(first) and _is_date(second) and first > second"
        cond_n = "_is_number(first) and _is_number(second) and first > second"
        fe = _finding_expr(r, rng, "first")
        if kinds == "both":
            out += ["if (%s) or (%s):" % (cond_d, cond_n), "    return " + fe, "return None"]
        else:
            out += ["if %s:" % cond_d, "    return " + fe, "if %s:" % cond_n, "    return " + fe]
        return out
    var = rng.choice(["value", "value", f.split("_")[0]])
    if var in ("record", "first", "second", "re", "datetime", "ctx"):
        var = "value"
    cond, alt = _cond(r, var)
    if t in ("not_future", "max_age", "strict_required") and rng.random() < 0.5:
        if t == "not_future":
            cond = "_in_future(%s)" % var
        elif t == "max_age":
            cond = "_too_old(%s, %d)" % (var, r["days"])
        else:
            cond, alt = cond.replace("STRICT", "_strict()"), alt.replace("STRICT", "_strict()")
    fe = _finding_expr(r, rng, var)
    style = rng.choice(["guard", "inline", "var", "alt"] if alt else ["guard", "inline", "var"])
    if style == "alt":
        cond_alt = alt.replace(var, 'record.get("%s")' % f)
        out += ["if %s:" % cond_alt, "    return " + fe]
        if rng.random() < 0.5:
            out.append("return None")
        if r["valmsg"]:
            raise AssertionError("valmsg with alt")
        return out
    out.append('%s = record.get("%s")' % (var, f))
    if style == "var":
        return ["finding = None"] + out + ["if %s:" % cond, "    finding = " + fe, "return finding"]
    if style == "guard" and r["tpl"] not in ("required", "strict_required", "not_future", "max_age", "not_blank",
                                             "precision", "lowercase"):
        out += ["if %s is None:" % var, "    return None"]
        cond = cond.replace("%s is not None and " % var, "", 1)
    out += ["if %s:" % cond, "    return " + fe]
    if rng.random() < 0.75:
        out.append("return None")
    return out


def _composed_body(r, rng, byid):
    subs = [byid[s]["fn"] for s in r["subs"]]
    style = rng.choice(["loop", "chain", "registry", "ifs", "helper", "helper"])
    if r.get("force_style"):
        style = r["force_style"]
    if style == "helper":
        call = "return _first(record, %s)" % ", ".join(subs)
        if len(call) > 100:
            return ["return _first(", "    record,"] + ["    %s," % x for x in subs] + [")"]
        return [call]
    if style == "loop":
        return ["for rule in (%s):" % ", ".join(subs) + ("" if len(subs) > 1 else ""),
                "    finding = rule(record)", "    if finding is not None:", "        return finding",
                "return None"]
    if style == "chain":
        out = ["return (%s(record)" % subs[0]]
        out += ["        or %s(record)" % s for s in subs[1:]]
        out[-1] += ")"
        return out
    if style == "registry":
        return ["for rule_id in (%s):" % ", ".join('"%s"' % s for s in r["subs"]),
                "    finding = RULES[rule_id][0](record)", "    if finding is not None:", "        return finding",
                "return None"]
    out = []
    for k, s in enumerate(subs):
        out += ["finding = %s(record)" % s, "if finding is not None:", "    return finding"]
    out.append("return None")
    return out


def render_rules(cat):
    rng = random.Random(SEED + 1)
    byid = by_id(cat)
    # composed rules that reference a retired rule: one by registry lookup, one by name
    ref_retired = [r for r in cat["rules"] if r["tpl"] == "composed" and any(s in cat["retired"] for s in r["subs"])]
    for k, r in enumerate(ref_retired):
        r["force_style"] = "registry" if k % 2 == 0 else "loop"
    lines = HEADER.rstrip("\n").split("\n")
    singles = dict((s[0], s[1]) for s in SECTIONS)
    sec = None
    for r in cat["rules"]:
        if r["section"] != sec:
            sec = r["section"]
            lines += ["", "", "# " + "=" * 70 + " " + sec]
        lines += ["", ""]
        if rng.random() < 0.12:
            lines.append(rng.choice(["# catalogue owner: data-quality", "# reviewed in the 2025 audit",
                                     "# keep the message text stable: reports match on it"]))
        lines.append("def %s(record):" % r["fn"])
        lines += [("    " + x) if x else "" for x in _doc(r, rng, singles[sec], byid)]
        body = _composed_body(r, rng, byid) if r["tpl"] == "composed" else _plain_body(r, rng)
        lines += ["    " + x for x in body]
    lines += ["", "", "# rule id -> (function, category)"]
    ids = [r["id"] for r in cat["rules"]]
    half = len(ids) // 2
    lines.append("RULES = {")
    for i, rid in enumerate(ids):
        if i == half:
            lines += ["}", "", "# rules added after the 2025 audit", "RULES.update({"]
        r = byid[rid]
        if rng.random() < 0.1:
            lines += ['    "%s": (' % rid, '        %s, "%s"),' % (r["fn"], r["cat"])]
        else:
            lines.append('    "%s": (%s, "%s"),' % (rid, r["fn"], r["cat"]))
    lines.append("})")
    return "\n".join(lines) + "\n"


# ------------------------------------------------------------------------------------------ MIGRATION.md


MIGRATION = """# Rule table: v1 to v2 migration

`rules.py` holds the generated v1 rule table: %(count)d rule functions in %(nsec)d sections (%(sections)s)
and the registry `RULES` (rule id -> `(function, category)`) at the end of the file. The generator is gone, so the file
is migrated in place. Apply the rules below to every rule function, except where the exceptions table says
otherwise. What this document does not change stays as it is: function names, the conditions each rule
checks, the messages, and the categories in `RULES`.

## Terms

- **Rule function**: a function registered in `RULES`. Its rule id is its key in `RULES` (most docstrings
  also start with it); its category is the second item of its `RULES` entry.
- **Composed rule**: a rule function that runs other rule functions (its sub-rules) and returns the first
  finding; all others are plain rules.
- **Context**: `ctx`, a dict with the keys `today` (an ISO date string, `YYYY-MM-DD`) and `strict` (a bool).

## Rule 1: signature

Every rule function becomes `def <name>(record, ctx):` (two positional parameters, no defaults). Signatures are
read from the source.

## Rule 2: findings carry the rule id

A rule returns `None` when the record passes, or a 4-tuple `(rule_id, code, severity, message)`: `rule_id` is
the id of the rule whose check failed, `code` and `severity` follow rules 3 and 4, `message` is the v1 message
unchanged. A composed rule returns its sub-rule's finding as it is (it carries the sub-rule's id).

## Rule 3: codes

The category is the one of the rule whose check failed (for a merged rule's check, the survivor's; see rule 6).

| v1 code | rule category | v2 code |
|---|---|---|
%(codes)s

## Rule 4: severities

v2 has four severities. A v1 `error` becomes `blocker` when the rule's category is `money` or `identity` and
stays `error` otherwise; a v1 `warning` becomes `info` when the rule's category is `style` and stays `warning`
otherwise.

## Rule 5: context instead of module settings

Rules read today's date and strict mode from `ctx` (`ctx["today"]`, `ctx["strict"]`), never from the module's
`TODAY` and `STRICT`, whether directly or through a helper function. A composed rule passes the `ctx` it
received to every sub-rule it runs.

## Rule 6: retired and merged rules

Retired rules are replaced by another rule: the old function stays (other code imports it) as a forwarding
shim, `def <old>(record, ctx)` returning what the replacement returns for the same arguments.

| Retired | Replaced by |
|---|---|
%(retired)s

Merged rules fold one rule (the merged one) into another (the survivor). The survivor runs its own check
first; when that passes it runs the merged rule's check, and a finding from that check carries the
survivor's id and the merged rule's message, with the merged rule's v1 code and severity mapped by rules 3
and 4 under the survivor's category. The merged rule's function stays as a forwarding shim to the
survivor.

| Merged | Into |
|---|---|
%(merged)s

The ids of retired and merged rules leave `RULES`; every other entry stays as it is. A composed rule that
names a retired or merged rule keeps working: it runs the replacement (or survivor) in its place, including
when it looks the id up in `RULES`.

## Exceptions

These rules differ from the rules above in exactly the way stated; every other rule still applies to them.

| Rule id | What differs |
|---|---|
%(table)s
"""


def code_rows():
    rows = []
    for c, v in CODES:
        special = sorted((k, w) for (x, k), w in CODE_BY_CAT.items() if x == c)
        rows += ["| `%s` | `%s` | `%s` |" % (c, k, w) for k, w in special]
        rows.append("| `%s` | %s | `%s` |" % (c, "any other" if special else "any", v))
    return rows


def migration_md(cat):
    byid = by_id(cat)
    rows = []
    for rid in cat["keep_code"]:
        rows.append((rid, "keeps its v1 code (`%s`)" % byid[rid]["code"]))
    for rid, sev in sorted(cat["pinned"].items()):
        rows.append((rid, "severity is always `%s`" % sev))
    for rid in cat["strict_sub"]:
        rows.append((rid, "runs its sub-rules with strict mode on: passes `dict(ctx, strict=True)` instead of `ctx`"))
    rows.sort()
    return MIGRATION % {
        "count": len(cat["rules"]), "nsec": len(SECTION_NAMES), "sections": ", ".join(SECTION_NAMES),
        "codes": "\n".join(code_rows()),
        "retired": "\n".join("| `%s` | `%s` |" % kv for kv in sorted(cat["retired"].items())),
        "merged": "\n".join("| `%s` | `%s` |" % kv for kv in sorted(cat["merged"].items())),
        "table": "\n".join("| `%s` | %s |" % kv for kv in rows)}


# ------------------------------------------------------------------------------------------ visible tests


TEST_HEAD = '''"""Examples of the v2 behaviour of rules.py (see MIGRATION.md).

Run from the workspace root: /usr/bin/python3 -m unittest discover -s tests -v
These cover a few rules only; the migration applies to every rule function.
"""
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import rules  # noqa: E402

CTX = {"today": "%s", "strict": False}


class RulesV2Test(unittest.TestCase):
    def check(self, fn, record, ctx, expected):
        result = getattr(rules, fn)(dict(record), dict(ctx))
        self.assertEqual(None if result is None else tuple(result), expected)
''' % TODAY


def visible_tests(cat):
    byid = by_id(cat)
    secs = dict((s[0], s) for s in SECTIONS)
    special = set(cat["keep_code"]) | set(cat["pinned"]) | set(cat["strict_sub"]) | set(cat["retired"]) \
        | set(cat["retired"].values()) | set(cat["merged"]) | set(cat["merged"].values())
    out = [TEST_HEAD]
    k = 0
    for s in SECTION_NAMES[:3]:
        rs = [r for r in cat["rules"] if r["section"] == s and r["id"] not in special]
        picks = []
        for cond in (lambda r: r["tpl"] in ("pattern", "range", "positive") and r["sev"] == "error",
                     lambda r: r["tpl"] in ("max_len", "one_of", "date") and r["sev"] == "warning",
                     lambda r: r["tpl"] in ("not_future", "max_age", "strict_required"),
                     lambda r: r["tpl"] == "composed"):
            for r in rs:
                if cond(r) and r not in picks:
                    picks.append(r)
                    break
        for r in picks:
            k += 1
            recs = probe_records(cat, r, secs[s], 50 + k)
            out.append("    def test_%s_%s(self):" % (r["id"].lower(), r["fn"]))
            ctxs = [ALT_CTX[0]] + ([ALT_CTX[1 + k % 3]] if r["tpl"] != "required" else [])
            shown = 0
            for rec in recs[1:3] + recs[:1]:
                for ctx in ctxs:
                    exp = v2(cat, r["id"], rec, ctx, byid)
                    out.append("        self.check(%r, %r,\n                   %r,\n                   %r)" % (
                        r["fn"], rec, ctx, exp))
                    shown += 1
                    if shown >= 4:
                        break
                if shown >= 4:
                    break
            out.append("")
    old, new = sorted(cat["retired"].items())[0]
    r, rn = byid[old], byid[new]
    rec = probe_records(cat, rn, secs[rn["section"]], 77)[1]
    out.append("    def test_retired_%s(self):" % old.lower())
    out.append("        self.check(%r, %r,\n                   %r,\n                   %r)" % (
        r["fn"], rec, ALT_CTX[0], v2(cat, old, rec, ALT_CTX[0], byid)))
    out.append("        self.assertNotIn(%r, rules.RULES)" % old)
    out.append("")
    out.append("    def test_registry_entry(self):")
    rid = [x for x in live_ids(cat) if byid[x]["section"] == "invoices"][0]
    out.append("        self.assertEqual(rules.RULES[%r], (rules.%s, %r))" % (rid, byid[rid]["fn"], byid[rid]["cat"]))
    out.append("")
    out.append("")
    out.append('if __name__ == "__main__":')
    out.append("    unittest.main()")
    return "\n".join(out) + "\n"


def write_workspace(ws):
    cat = build()
    text = render_rules(cat)
    with open(os.path.join(ws, "rules.py"), "w") as f:
        f.write(text)
    with open(os.path.join(ws, "MIGRATION.md"), "w") as f:
        f.write(migration_md(cat))
    os.makedirs(os.path.join(ws, "tests"), exist_ok=True)
    with open(os.path.join(ws, "tests", "test_rules.py"), "w") as f:
        f.write(visible_tests(cat))


if __name__ == "__main__":
    write_workspace(sys.argv[1])
