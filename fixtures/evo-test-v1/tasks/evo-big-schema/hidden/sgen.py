"""evo-big-schema: generator of the initial workspace, and the behaviour model the check uses.

As a script (`sgen.py WS`, run by workspace.gen) it writes WS/models.py (the generated v1 record classes),
WS/MIGRATION.md (the v1 -> v2 rules and the exceptions table) and WS/tests/test_models.py (visible examples).
Imported, build() returns the catalogue and the v2_* functions give the migrated behaviour.
Deterministic: one seeded random.Random, no clock, no environment. Python 3.9, stdlib only.
"""
import os
import random
import sys

SEED = 9157
UNITS = {"mi": ("km", 1.609344), "lb": ("kg", 0.45359237)}
ENUMS = {
    "status": [("open", "active"), ("closed", "done"), ("void", "cancelled"), ("hold", "paused")],
    "priority": [("lo", "low"), ("mid", "normal"), ("hi", "high")],
    "mode": [("auto", "automatic"), ("man", "manual")],
}
SECTIONS = [
    ("vehicles", ["Vehicle", "Trailer", "Engine", "Axle"]),
    ("trips", ["Trip", "Leg", "Stop", "Detour"]),
    ("drivers", ["Driver", "Shift", "License", "Break"]),
    ("sensors", ["Sensor", "Reading", "Probe", "Gauge"]),
    ("maintenance", ["Repair", "Inspection", "Part", "Warranty"]),
    ("fuel", ["FuelCard", "Refill", "Tank", "Station"]),
    ("routes", ["Route", "Waypoint", "Zone", "Corridor"]),
    ("depots", ["Depot", "Bay", "Dock", "Yard"]),
    ("alerts", ["Alert", "Incident", "Notice", "Escalation"]),
    ("cargo", ["Load", "Pallet", "Container", "Manifest"]),
]
SECTION_NAMES = [s[0] for s in SECTIONS]
SUFFIXES = ["", "Status", "Summary", "Event", "Snapshot", "Spec", "Update", "Report", "Note", "Link", "Batch",
            "Request", "Window", "Ref"]
PLAIN = {"label": "str", "note": "str", "code": "str", "region": "str", "source": "str", "created_at": "str",
         "ref": "str", "count": "int", "seq": "int", "axles": "int", "amount": "float", "score": "float",
         "cost": "float", "active": "bool", "flagged": "bool"}
UNIT_FIELDS = ["distance_mi", "odometer_mi", "range_mi", "weight_lb", "payload_lb", "tare_lb"]
IRREGULAR = {"note": "remark", "label": "displayName", "code": "shortCode", "region": "area", "amount": "value",
             "count": "total", "seq": "sequence", "score": "rating", "cost": "price", "active": "enabled",
             "flagged": "isFlagged", "ref": "externalRef", "created_at": "createdOn", "source": "origin"}


def camel(name):
    head, *rest = name.split("_")
    return head + "".join(x.capitalize() for x in rest)


def snake(name):
    out = ""
    for ch in name:
        out += ("_" + ch.lower()) if ch.isupper() else ch
    return out.lstrip("_")


def unit_of(attr):
    for u in UNITS:
        if attr.endswith("_" + u):
            return u
    return None


def F(attr, kind, required, default=None, v1key=None):
    return {"attr": attr, "kind": kind, "req": required, "default": default, "v1key": v1key or camel(attr)}


def _fields(rng, noun):
    idf = snake(noun) + "_id"
    fs = [F(idf, "str", True)]
    plain = sorted(PLAIN)
    rng.shuffle(plain)
    k = 0
    if rng.random() < 0.6:
        a = plain[k]
        k += 1
        fs.append(F(a, PLAIN[a], True))
    if rng.random() < 0.5:
        u = rng.choice(UNIT_FIELDS)
        fs.append(F(u, "unit", rng.random() < 0.3))
        if rng.random() < 0.3:
            u2 = rng.choice([x for x in UNIT_FIELDS if x != u])
            fs.append(F(u2, "unit", False))
    opt = []
    for a in plain[k:k + rng.choice([2, 3, 3, 4])]:
        opt.append(F(a, PLAIN[a], False, False if PLAIN[a] == "bool" and rng.random() < 0.5 else None))
    if rng.random() < 0.55:
        e = rng.choice(sorted(ENUMS))
        opt.insert(rng.randint(0, len(opt)), F(e, "enum", False, ENUMS[e][0][0]))
    req = [f for f in fs if f["req"]]
    rest = [f for f in fs if not f["req"]] + opt
    out = req + rest
    for f in out[1:]:
        if f["kind"] != "unit" and rng.random() < 0.15:
            f["v1key"] = IRREGULAR.get(f["attr"], f["v1key"])
    if rng.random() < 0.1:
        out[0]["v1key"] = camel(out[0]["attr"][:-3])
    return out


def build():
    """{classes: [class dicts in file order], legacy: [names], imperial: [names], enumkeep: [names]}"""
    rng = random.Random(SEED)
    classes = []
    used = set()
    for sname, nouns in SECTIONS:
        names = []
        for noun in nouns:
            for suf in SUFFIXES:
                names.append((noun, noun + suf))
        rng.shuffle(names)
        mine = []
        for noun, cname in names[:20]:
            used.add(cname)
            mine.append({"name": cname, "section": sname, "noun": noun, "fields": _fields(rng, noun), "dep": None})
        for rep in rng.sample(mine, 2):
            old = "Legacy" + rep["name"] if rng.random() < 0.5 else rep["name"] + "V1"
            assert old not in used
            used.add(old)
            mine.insert(rng.randint(0, len(mine)), {"name": old, "section": sname, "noun": rep["noun"],
                                                    "fields": [dict(f) for f in rep["fields"]], "dep": rep["name"]})
        classes += mine
    cat = {"classes": classes, "legacy": [], "imperial": [], "enumkeep": []}
    reps = set(c["dep"] for c in classes if c["dep"])
    free = [c for c in classes if not c["dep"] and c["name"] not in reps]
    rng.shuffle(free)
    taken = set()

    def pick(cond, n):
        got, secs = [], set()
        for c in free:
            if c["name"] in taken or c["section"] in secs or not cond(c):
                continue
            got.append(c["name"])
            secs.add(c["section"])
            taken.add(c["name"])
            if len(got) == n:
                return sorted(got)
        raise AssertionError("not enough candidates")
    cat["legacy"] = pick(lambda c: not any(f["kind"] == "unit" for f in c["fields"]), 4)
    cat["imperial"] = pick(lambda c: any(f["kind"] == "unit" for f in c["fields"]), 3)
    cat["enumkeep"] = pick(lambda c: any(f["kind"] == "enum" for f in c["fields"]), 3)
    return cat


# ------------------------------------------------------------------------------------------ model


def by_name(cat):
    return dict((c["name"], c) for c in cat["classes"])


def v2_attr(c, f, cat):
    u = unit_of(f["attr"]) if f["kind"] == "unit" else None
    if u and c["name"] not in cat["imperial"]:
        return f["attr"][:-len(u)] + UNITS[u][0]
    return f["attr"]


def v2_key(c, f, cat):
    return f["v1key"] if c["name"] in cat["legacy"] else v2_attr(c, f, cat)


def enum_map(c, f, cat, v):
    if f["kind"] != "enum" or c["name"] in cat["enumkeep"]:
        return v
    return dict(ENUMS[f["attr"]]).get(v, v)


def v2_default(c, f, cat):
    return enum_map(c, f, cat, f["default"])


def v1_sig(c):
    return [(f["attr"], not f["req"], None if f["req"] else f["default"]) for f in c["fields"]]


def v2_sig(c, cat):
    return [(v2_attr(c, f, cat), not f["req"], None if f["req"] else v2_default(c, f, cat)) for f in c["fields"]]


def v2_to_dict(c, cat, kw):
    """to_dict() of an instance built with kw (v2 parameter names)."""
    out = {}
    for f in c["fields"]:
        a = v2_attr(c, f, cat)
        v = kw[a] if a in kw else v2_default(c, f, cat)
        if v is not None:
            out[v2_key(c, f, cat)] = v
    return out


def convert(c, f, cat, v):
    u = unit_of(f["attr"]) if f["kind"] == "unit" else None
    if u and c["name"] not in cat["imperial"] and v is not None and not isinstance(v, bool):
        return round(v * UNITS[u][1], 3)
    return v


def v2_from_dict(c, cat, data):
    """attributes (v2 names) after from_dict(data), or ("raise", message)."""
    attrs = {}
    for f in c["fields"]:
        a = v2_attr(c, f, cat)
        k2 = v2_key(c, f, cat)
        if k2 in data:
            v = data[k2]
        elif f["v1key"] in data:
            v = convert(c, f, cat, data[f["v1key"]])
        elif f["req"]:
            return ("raise", "%s.from_dict: missing %s" % (c["name"], a))
        else:
            v = v2_default(c, f, cat)
        attrs[a] = enum_map(c, f, cat, v)
    return attrs


def dep_message(c):
    return "%s is deprecated; use %s instead" % (c["name"], c["dep"])


def sample(f, salt, tag, v1=False):
    k = f["kind"]
    if k == "enum":
        vals = ENUMS[f["attr"]]
        pair = vals[salt % len(vals)]
        return pair[0] if v1 else pair[1]
    if k == "unit":
        return round(1.5 + (salt * 7.25) % 400, 2)
    t = f["kind"]
    if t == "int":
        return 1 + (salt * 13) % 90
    if t == "float":
        return round(0.5 + (salt * 3.75) % 200, 2)
    if t == "bool":
        return salt % 2 == 0
    return "%s%d-%s" % (tag, salt, f["attr"][:3])


def payload_v2(c, cat, salt, tag, full=True):
    d = {}
    for i, f in enumerate(c["fields"]):
        if f["req"] or full:
            d[v2_key(c, f, cat)] = sample(f, salt + i, tag, v1=c["name"] in cat["enumkeep"])
    return d


def payload_v1(c, cat, salt, tag, full=True):
    d = {}
    for i, f in enumerate(c["fields"]):
        if f["req"] or full:
            d[f["v1key"]] = sample(f, salt + i, tag, v1=True)
    return d


def kwargs_v2(c, cat, salt, tag, full=True):
    kw = {}
    for i, f in enumerate(c["fields"]):
        if f["req"] or full:
            kw[v2_attr(c, f, cat)] = sample(f, salt + i, tag, v1=c["name"] in cat["enumkeep"])
    return kw


# ------------------------------------------------------------------------------------------ render v1


HEADER = '''"""Fleet API record classes (v1 wire format).

Generated from the fleet schema: one class per record type. Each class takes its fields as constructor
arguments (stored as attributes of the same names), turns itself into the JSON-ready dict of the v1 wire
format with to_dict(), and is built back from such a dict with the from_dict() class method. Optional fields
whose value is None are left out of to_dict()'s result.

MIGRATION.md describes the move to the v2 wire format.
"""


class Record:
    """Base class: equality and repr over the attributes."""

    def __eq__(self, other):
        return type(self) is type(other) and vars(self) == vars(other)

    def __repr__(self):
        return "%s(%s)" % (type(self).__name__, ", ".join("%s=%r" % kv for kv in sorted(vars(self).items())))


def _compact(data):
    """Drop the keys whose value is None."""
    return {k: v for k, v in data.items() if v is not None}
'''


def _doc(c, rng):
    if rng.random() < 0.1:
        return ['"""%s record."""' % c["name"]]
    out = ['"""%s record (fleet API v1).' % c["name"], "", "Fields:"]
    for f in c["fields"]:
        kind = f["kind"]
        if kind == "enum":
            kind = "one of " + ", ".join(a for a, _ in ENUMS[f["attr"]])
        elif kind == "unit":
            kind = "float, " + ("miles" if f["attr"].endswith("_mi") else "pounds")
        out.append("    %s (%s%s): wire key %s" % (f["attr"], kind, ", required" if f["req"] else "", f["v1key"]))
    out.append('"""')
    return out


def _init(c, rng):
    parts = ["self"] + [f["attr"] if f["req"] else "%s=%r" % (f["attr"], f["default"]) for f in c["fields"]]
    one = "def __init__(%s):" % ", ".join(parts)
    if len(one) <= 96 and rng.random() < 0.75:
        head = [one]
    else:
        head = ["def __init__("] + ["    %s," % p for p in parts] + ["):"]
    return head + ["    self.%s = %s" % (f["attr"], f["attr"]) for f in c["fields"]]


def _to_dict(c, rng):
    style = rng.choice(["compact", "incr", "pairs"])
    out = ["def to_dict(self):"]
    if style == "compact":
        out.append("    return _compact({")
        out += ['        "%s": self.%s,' % (f["v1key"], f["attr"]) for f in c["fields"]]
        out.append("    })")
    elif style == "incr":
        req = [f for f in c["fields"] if f["req"]]
        out.append("    out = {%s}" % ", ".join('"%s": self.%s' % (f["v1key"], f["attr"]) for f in req))
        for f in c["fields"]:
            if not f["req"]:
                out += ["    if self.%s is not None:" % f["attr"], '        out["%s"] = self.%s' % (f["v1key"], f["attr"])]
        out.append("    return out")
    else:
        out.append("    pairs = (")
        out += ['        ("%s", self.%s),' % (f["v1key"], f["attr"]) for f in c["fields"]]
        out += ["    )", "    return {key: value for key, value in pairs if value is not None}"]
    return out


def _from_dict(c, rng):
    style = rng.choice(["kw", "kw", "pos", "loop"])
    out = ["@classmethod", "def from_dict(cls, data):"]
    if style == "loop" and any(not f["req"] for f in c["fields"]):
        req = [f for f in c["fields"] if f["req"]]
        out.append("    kwargs = {%s}" % ", ".join('"%s": data["%s"]' % (f["attr"], f["v1key"]) for f in req))
        out.append("    for key, name in (%s):" % ", ".join('("%s", "%s")' % (f["v1key"], f["attr"])
                                                       for f in c["fields"] if not f["req"]))
        out += ["        if key in data:", "            kwargs[name] = data[key]", "    return cls(**kwargs)"]
        return out
    args = []
    for f in c["fields"]:
        if f["req"]:
            e = 'data["%s"]' % f["v1key"]
            args.append(e if style == "pos" else "%s=%s" % (f["attr"], e))
        elif f["default"] is None:
            args.append('%s=data.get("%s")' % (f["attr"], f["v1key"]))
        else:
            args.append('%s=data.get("%s", %r)' % (f["attr"], f["v1key"], f["default"]))
    call = "    return cls(%s)" % ", ".join(args)
    if len(call) <= 100:
        return out + [call]
    return out + ["    return cls("] + ["        %s," % a for a in args] + ["    )"]


def render_models(cat):
    rng = random.Random(SEED + 1)
    lines = HEADER.rstrip("\n").split("\n")
    sec = None
    for c in cat["classes"]:
        if c["section"] != sec:
            sec = c["section"]
            lines += ["", "", "# " + "=" * 70 + " " + sec]
        lines += ["", ""]
        if c["dep"]:
            if rng.random() < 0.3:
                lines.append("# kept for old importers")
            lines.append("# deprecated: use %s" % c["dep"])
        lines.append("class %s(Record):" % c["name"])
        body = _doc(c, rng) + [""] + _init(c, rng) + [""] + _to_dict(c, rng) + [""] + _from_dict(c, rng)
        lines += [("    " + x) if x else "" for x in body]
    return "\n".join(lines) + "\n"


# ------------------------------------------------------------------------------------------ MIGRATION.md


MIGRATION = """# Fleet records: v1 to v2 wire format

`models.py` holds the generated v1 record classes: %(count)d classes in %(nsec)d sections (%(sections)s). The
generator is gone, so the file is migrated in place. Apply the rules below to every record class, except where
the exceptions table says otherwise. What this document does not change stays as it is: class names, field
order, the constructor's required and optional parameters, and `to_dict()` leaving out fields whose value is
`None`.

## Terms

- **Field**: a parameter of the class's `__init__`; the instance stores it as the attribute of the same name.
  A field without a default is **required**.
- **v1 wire key**: the key under which v1's `to_dict()` writes the field (and `from_dict()` reads it).
- **Unit field**: a field whose name ends in `_mi` (miles) or `_lb` (pounds).
- **Enum field**: a field named `status`, `priority` or `mode`.
- **Deprecated class**: a comment line `# deprecated: use NAME` sits directly above its `class` line (other
  comment lines may sit with it). `NAME` is its replacement, another class with the same fields.

## Rule 1: v2 wire keys

`to_dict()` writes every field under its v2 wire key: the field's (v2) name, in snake_case exactly as the
attribute is named. `from_dict()` reads the v2 wire keys.

## Rule 2: v1 payloads still load

`from_dict()` also accepts v1 payloads: for each field, when its v2 wire key is absent from the dict and its
v1 wire key is present, it uses the value under the v1 key (converted by rules 3 and 4).

## Rule 3: metric units

A unit field `<base>_mi` becomes `<base>_km` and `<base>_lb` becomes `<base>_kg`: the parameter, the
attribute and the v2 wire key are renamed, in the same position and with the same default. Values are metric
in v2. A value read from a v1 wire key is converted: miles to kilometres `round(v * 1.609344, 3)`, pounds to
kilograms `round(v * 0.45359237, 3)`; a value read from the v2 key is taken as it is.

## Rule 4: enum values

| Field | v1 value | v2 value |
|---|---|---|
%(enums)s

An enum field holds v2 values: the constructor's default becomes the v2 value of the v1 default, and
`from_dict()` turns any v1 value it reads (under either key) into its v2 value. `to_dict()` writes what the
attribute holds.

## Rule 5: missing required fields

`from_dict()` raises `ValueError("<Class>.from_dict: missing <field>")` (the field's v2 name) when a required
field is absent under both its v2 and its v1 wire key, checking the fields in order. Optional fields that are
absent take the constructor's default.

## Rule 6: deprecated classes

A deprecated class becomes a subclass of its replacement whose only member is an `__init__` with the
replacement's v2 parameters that emits exactly one warning, category `DeprecationWarning`, message exactly
`<Class> is deprecated; use <Replacement> instead`, and then initialises the instance as the replacement does
(same attributes, same `to_dict()`).

## Exceptions

These classes differ from the rules above in exactly the way stated; every other rule still applies to them.

| Class | What differs |
|---|---|
%(table)s
"""


def migration_md(cat):
    rows = []
    for n in cat["legacy"]:
        rows.append((n, "keeps the v1 wire keys: `to_dict()` writes and `from_dict()` reads the v1 keys (rule 1 "
                        "does not apply)"))
    for n in cat["imperial"]:
        rows.append((n, "keeps miles and pounds: no renaming and no conversion (rule 3 does not apply); its "
                        "v2 wire keys are the unchanged snake_case names"))
    for n in cat["enumkeep"]:
        rows.append((n, "keeps the v1 enum values (rule 4 does not apply)"))
    rows.sort()
    enums = []
    for k in sorted(ENUMS):
        enums += ["| `%s` | `%s` | `%s` |" % (k, a, b) for a, b in ENUMS[k]]
    return MIGRATION % {"count": len(cat["classes"]), "nsec": len(SECTION_NAMES),
                        "sections": ", ".join(SECTION_NAMES), "enums": "\n".join(enums),
                        "table": "\n".join("| `%s` | %s |" % kv for kv in rows)}


# ------------------------------------------------------------------------------------------ visible tests


TEST_HEAD = '''"""Examples of the v2 behaviour of models.py (see MIGRATION.md).

Run from the workspace root: /usr/bin/python3 -m unittest discover -s tests -v
These cover a few classes only; the migration applies to every record class.
"""
import os
import sys
import unittest
import warnings

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import models  # noqa: E402


class ModelsV2Test(unittest.TestCase):
    def roundtrip(self, name, kwargs, wire, v1_payload, attrs):
        cls = getattr(models, name)
        self.assertEqual(cls(**kwargs).to_dict(), wire)
        obj = cls.from_dict(dict(wire))
        self.assertEqual(obj.to_dict(), wire)
        old = cls.from_dict(dict(v1_payload))
        for attr, value in attrs.items():
            self.assertEqual(getattr(old, attr), value, attr)
'''


def visible_tests(cat):
    byn = by_name(cat)
    special = set(cat["legacy"]) | set(cat["imperial"]) | set(cat["enumkeep"])
    out = [TEST_HEAD]
    k = 0
    for s in SECTION_NAMES[:3]:
        cs = [c for c in cat["classes"] if c["section"] == s and not c["dep"] and c["name"] not in special]
        picks = []
        for cond in (lambda c: any(f["kind"] == "unit" for f in c["fields"]),
                     lambda c: any(f["kind"] == "enum" for f in c["fields"]),
                     lambda c: any(f["v1key"] != camel(f["attr"]) for f in c["fields"])):
            for c in cs:
                if cond(c) and c not in picks:
                    picks.append(c)
                    break
        for c in picks:
            k += 1
            salt = 500 + 19 * k
            kw = kwargs_v2(c, cat, salt, "v")
            wire = v2_to_dict(c, cat, kw)
            p1 = payload_v1(c, cat, salt + 50, "v")
            attrs = v2_from_dict(c, cat, p1)
            out.append("    def test_%s(self):" % snake(c["name"]))
            out.append("        self.roundtrip(%r,\n                       %r,\n                       %r,\n"
                       "                       %r,\n                       %r)" % (c["name"], kw, wire, p1, attrs))
            req = [f for f in c["fields"] if f["req"]]
            data = payload_v2(c, cat, salt, "v")
            del data[v2_key(c, req[0], cat)]
            msg = v2_from_dict(c, cat, data)[1]
            out.append("        with self.assertRaises(ValueError) as cm:")
            out.append("            models.%s.from_dict(%r)" % (c["name"], data))
            out.append("        self.assertEqual(str(cm.exception), %r)" % msg)
            out.append("")
        d = [c for c in cat["classes"] if c["section"] == s and c["dep"]][0]
        rep = byn[d["dep"]]
        kw = kwargs_v2(rep, cat, 600 + k, "v")
        out.append("    def test_deprecated_%s(self):" % snake(d["name"]))
        out.append("        with warnings.catch_warnings(record=True) as w:")
        out.append('            warnings.simplefilter("always")')
        out.append("            obj = models.%s(**%r)" % (d["name"], kw))
        out.append("        self.assertEqual([(x.category, str(x.message)) for x in w],")
        out.append("                         [(DeprecationWarning, %r)])" % dep_message(d))
        out.append("        self.assertIsInstance(obj, models.%s)" % rep["name"])
        out.append("        self.assertEqual(obj.to_dict(), %r)" % v2_to_dict(rep, cat, kw))
        out.append("")
    out.append("")
    out.append('if __name__ == "__main__":')
    out.append("    unittest.main()")
    return "\n".join(out) + "\n"


def write_workspace(ws):
    cat = build()
    with open(os.path.join(ws, "models.py"), "w") as f:
        f.write(render_models(cat))
    with open(os.path.join(ws, "MIGRATION.md"), "w") as f:
        f.write(migration_md(cat))
    os.makedirs(os.path.join(ws, "tests"), exist_ok=True)
    with open(os.path.join(ws, "tests", "test_models.py"), "w") as f:
        f.write(visible_tests(cat))


if __name__ == "__main__":
    write_workspace(sys.argv[1])
