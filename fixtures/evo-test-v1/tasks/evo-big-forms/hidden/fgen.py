"""evo-big-forms: generator of the initial workspace, and the behaviour model the check uses.

As a script (`fgen.py WS`, run by workspace.gen) it writes WS/widgets.py (the generated v1 form-widget
registry), WS/MIGRATION.md (the v1 -> v2 rules and the exceptions table) and WS/tests/test_widgets.py (visible
examples). Imported, build() returns the catalogue and v2_tokens() gives the migrated behaviour.
Deterministic: one seeded random.Random, no clock, no environment. Python 3.9, stdlib only.
"""
import os
import random
import re
import sys

SEED = 6211
NAME_RE = r"[a-z][a-z0-9_]{0,39}"
RENAME = {"placeholder": "hint", "maxlength": "max-length", "readonly": "read-only", "autofocus": "focus",
          "accept": "accept-types", "autocomplete": "fill"}
# parameter -> (attribute key, kind, default choices); kind: str | int | bool
OPTIONS = {
    "label": ("label", "str", [None]), "placeholder": ("placeholder", "str", [None]),
    "max_length": ("maxlength", "int", [80, 120, 254]), "min_value": ("min", "int", [None, 0]),
    "max_value": ("max", "int", [None, 100]), "step": ("step", "int", [1, 5]), "rows": ("rows", "int", [3, 4, 6]),
    "accept": ("accept", "str", [None, "image/*"]), "pattern": ("pattern", "str", [None]),
    "size": ("size", "int", [20, 30]), "autocomplete": ("autocomplete", "str", [None, "off"]),
    "required": ("required", "bool", [False]), "disabled": ("disabled", "bool", [False]),
    "readonly": ("readonly", "bool", [False]), "autofocus": ("autofocus", "bool", [False]),
    "multiple": ("multiple", "bool", [False]), "checked": ("checked", "bool", [False]),
}
KINDS = {
    "text": ("input:text", ["placeholder", "max_length", "pattern", "autocomplete", "size", "readonly"]),
    "email": ("input:email", ["placeholder", "max_length", "autocomplete", "multiple", "readonly"]),
    "number": ("input:number", ["min_value", "max_value", "step", "readonly"]),
    "date": ("input:date", ["min_value", "max_value", "readonly"]),
    "select": ("select", ["multiple", "size", "autocomplete"]),
    "area": ("textarea", ["placeholder", "max_length", "rows", "readonly"]),
    "check": ("input:checkbox", ["checked"]),
    "file": ("input:file", ["accept", "multiple"]),
    "password": ("input:password", ["placeholder", "max_length", "autocomplete", "pattern"]),
    "range": ("input:range", ["min_value", "max_value", "step"]),
    "phone": ("input:tel", ["placeholder", "pattern", "autocomplete", "size"]),
}
SECTIONS = [
    ("account", ["email", "username", "display_name", "password", "recovery", "avatar", "timezone", "language"]),
    ("billing", ["card_holder", "card_number", "iban", "vat", "invoice_email", "budget", "currency", "plan"]),
    ("shipping", ["street", "city", "postcode", "country", "phone", "parcel_weight", "delivery_date", "notes"]),
    ("profile", ["bio", "website", "birthday", "pronouns", "photo", "headline", "skills", "visibility"]),
    ("search", ["query", "category", "min_price", "max_price", "sort", "radius", "since", "tags"]),
    ("admin", ["role", "quota", "expiry", "audit_note", "api_scope", "ip_range", "retention", "export_format"]),
    ("survey", ["rating", "comment", "nps", "visit_date", "consent", "topic", "age_band", "attachment"]),
    ("support", ["subject", "message", "severity", "screenshot", "callback", "order_ref", "product", "contact_email"]),
]
SECTION_NAMES = [s[0] for s in SECTIONS]
COMMON = ["label", "required", "disabled", "autofocus"]


def build():
    """{widgets: [...], legacy: [names], barebools: [names]}"""
    rng = random.Random(SEED)
    widgets = []
    used = set()
    for sname, topics in SECTIONS:
        mine = []
        for topic in topics:
            for kind in rng.sample(sorted(KINDS), 4):
                name = "%s_%s" % (topic, kind)
                if len(mine) >= 27 or name in used:
                    continue
                used.add(name)
                tag, opts = KINDS[kind]
                params = [{"name": "name", "role": "name", "req": True}]
                if rng.random() < 0.25:
                    params.append({"name": "group_name", "role": "name", "req": True})
                chosen = [o for o in opts if rng.random() < 0.75] + [o for o in COMMON if rng.random() < 0.7]
                rng.shuffle(chosen)
                for o in chosen:
                    key, typ, defaults = OPTIONS[o]
                    params.append({"name": o, "role": "opt", "req": False, "key": key, "type": typ,
                                   "default": rng.choice(defaults)})
                kw = rng.random() < 0.25
                for p in params:
                    p["kw"] = kw and not p["req"]
                mine.append({"name": name, "section": sname, "tag": tag, "params": params, "dep": None})
        for rep in rng.sample(mine, 3):
            old = rep["name"] + "_v1" if rng.random() < 0.5 else "old_" + rep["name"]
            used.add(old)
            mine.insert(rng.randint(0, len(mine)), {"name": old, "section": sname, "tag": rep["tag"],
                                                    "params": [dict(p) for p in rep["params"]], "dep": rep["name"]})
        widgets += mine
    cat = {"widgets": widgets, "legacy": [], "barebools": []}
    reps = set(w["dep"] for w in widgets if w["dep"])
    free = [w for w in widgets if not w["dep"] and w["name"] not in reps]
    rng.shuffle(free)
    taken = set()

    def pick(cond, n):
        got, secs = [], set()
        for w in free:
            if w["name"] in taken or w["section"] in secs or not cond(w):
                continue
            got.append(w["name"])
            secs.add(w["section"])
            taken.add(w["name"])
            if len(got) == n:
                return sorted(got)
        raise AssertionError("not enough")
    cat["legacy"] = pick(lambda w: any(p.get("key") in RENAME for p in w["params"]), 3)
    cat["barebools"] = pick(lambda w: any(p.get("type") == "bool" for p in w["params"]), 3)
    return cat


# ------------------------------------------------------------------------------------------ model


def by_name(cat):
    return dict((w["name"], w) for w in cat["widgets"])


def key2(w, p, cat):
    if w["name"] in cat["legacy"]:
        return p["key"]
    return RENAME.get(p["key"], p["key"])


def v1_sig(w):
    return [(p["name"], p["kw"], not p["req"], None if p["req"] else p["default"]) for p in w["params"]]


def v2_sig(w):
    return v1_sig(w) + [("theme", True, True, None)]


def fmt(v):
    return str(v)


def name_key(p):
    return "name" if p["name"] == "name" else p["name"][:-5]


def v2_tokens(w, cat, kw):
    """("render", [tag, attrs...]) or ("raise", message) for a non-deprecated widget called with kw."""
    vals = {}
    for p in w["params"]:
        vals[p["name"]] = kw[p["name"]] if p["name"] in kw else (None if p["req"] else p["default"])
    for p in w["params"]:
        if p["role"] == "name":
            v = vals[p["name"]]
            if not (isinstance(v, str) and re.fullmatch(NAME_RE, v)):
                return ("raise", "%s: invalid %s: %r" % (w["name"], p["name"], v))
    toks = [w["tag"]]
    for p in w["params"]:
        v = vals[p["name"]]
        if p["role"] == "name":
            toks.append("%s=%s" % (name_key(p), v))
        elif p["type"] == "bool":
            if w["name"] in cat["barebools"]:
                if v:
                    toks.append(key2(w, p, cat))
            else:
                toks.append("%s=%s" % (key2(w, p, cat), "true" if v else "false"))
        elif v is not None:
            toks.append("%s=%s" % (key2(w, p, cat), fmt(v)))
    theme = kw.get("theme")
    if theme is not None:
        toks.append("theme=%s" % theme)
    return ("render", toks)


def v1_tokens(w, kw):
    vals = {}
    for p in w["params"]:
        vals[p["name"]] = kw[p["name"]] if p["name"] in kw else (None if p["req"] else p["default"])
    toks = [w["tag"]]
    for p in w["params"]:
        v = vals[p["name"]]
        if p["role"] == "name":
            toks.append("%s=%s" % (name_key(p), v))
        elif p["type"] == "bool":
            if v:
                toks.append(p["key"])
        elif v is not None and v != p["default"]:
            toks.append("%s=%s" % (p["key"], fmt(v)))
    return toks


def dep_message(w):
    return "%s() is deprecated; use %s() instead" % (w["name"], w["dep"])


def sample(p, salt, tag, other=True):
    """A valid value; other=True gives a value different from the default."""
    if p["role"] == "name":
        return "%s%d_%s" % (tag, salt, p["name"][:2])
    t = p["type"]
    if t == "bool":
        return other
    if not other:
        return p["default"]
    if t == "int":
        v = 1 + (salt * 7) % 300
        return v + 1 if v == p["default"] else v
    return "%s%d-%s" % (tag, salt, p["name"][:3])


def kwargs(w, salt, tag, mode):
    """mode: full (options set, not default), min (required only), defaults (options = defaults, bools False)."""
    kw = {}
    for i, p in enumerate(w["params"]):
        if p["req"]:
            kw[p["name"]] = sample(p, salt + i, tag)
        elif mode == "full":
            kw[p["name"]] = sample(p, salt + i, tag, True)
        elif mode == "defaults":
            kw[p["name"]] = False if p["type"] == "bool" else p["default"]
    return kw


# ------------------------------------------------------------------------------------------ render v1


HEADER = '''"""Form widget registry (v1).

Generated from the design system: one function per form widget. A widget function takes the field's name
and its options and hands a list of tokens to the renderer: first the element (for example "input:email"),
then one token per attribute ("key=value", or a bare "key" for a boolean attribute that is on). Options that
are None, or still at their default, are left out. The function returns what the renderer returns.

WIDGETS maps every widget name to its function. MIGRATION.md describes the move to the v2 tokens.
"""


class _Renderer:
    def render(self, tokens):
        return "<%s>" % " ".join(tokens)


RENDERER = _Renderer()
WIDGETS = {}


def widget(kind, section=None):
    """Register a widget function (metadata only)."""
    def deco(fn):
        fn.__widget__ = {"kind": kind, "section": section}
        WIDGETS[fn.__name__] = fn
        return fn
    return deco


def _emit(tokens):
    return RENDERER.render(tokens)


def _opt(tokens, key, value):
    """Append key=value when value is not None."""
    if value is not None:
        tokens.append("%s=%s" % (key, value))
'''


def _sig(w, rng):
    parts = []
    star = False
    for p in w["params"]:
        if p["kw"] and not star:
            parts.append("*")
            star = True
        parts.append(p["name"] if p["req"] else "%s=%r" % (p["name"], p["default"]))
    one = "def %s(%s):" % (w["name"], ", ".join(parts))
    if len(one) <= 96 and rng.random() < 0.7:
        return [one]
    return ["def %s(" % w["name"]] + ["    %s," % x for x in parts] + ["):"]


def _doc(w, rng):
    if rng.random() < 0.1:
        return ['"""%s widget."""' % w["name"].replace("_", " ").capitalize()]
    out = ['"""%s widget (%s).' % (w["name"].replace("_", " ").capitalize(), w["tag"]), "", "Args:"]
    for p in w["params"]:
        if p["role"] == "name":
            out.append("    %s: the field name (rendered as %s=...)." % (p["name"], name_key(p)))
        elif p["type"] == "bool":
            out.append("    %s: renders the bare %s attribute when true." % (p["name"], p["key"]))
        else:
            out.append("    %s: rendered as %s=... unless None or the default." % (p["name"], p["key"]))
    if rng.random() < 0.6:
        out += ["", "Returns:", "    What the renderer returns for the tokens."]
    out.append('"""')
    return out


def _value_expr(p, rng):
    k, n = p["key"], p["name"]
    if p["type"] == "int":
        return rng.choice(['"%s=%%d" %% %s' % (k, n), '"%s=" + str(%s)' % (k, n), 'f"%s={%s}"' % (k, n)])
    return rng.choice(['"%s=%%s" %% %s' % (k, n), '"%s=" + %s' % (k, n), 'f"%s={%s}"' % (k, n)])


def _body(w, rng):
    out = []
    head = ['"%s"' % w["tag"]]
    for p in w["params"]:
        if p["role"] == "name":
            head.append(rng.choice(['"%s=" + %s' % (name_key(p), p["name"]), 'f"%s={%s}"' % (name_key(p), p["name"])]))
    out.append("tokens = [%s]" % ", ".join(head))
    var = "tokens"
    bools = [p for p in w["params"] if p["role"] == "opt" and p["type"] == "bool"]
    grouped = len(bools) >= 2 and rng.random() < 0.4
    if grouped:
        out.append("tokens.extend(key for key, on in (%s) if on)" % ", ".join(
            '("%s", %s)' % (p["key"], p["name"]) for p in bools))
    for p in w["params"]:
        if p["role"] == "name":
            continue
        n, k = p["name"], p["key"]
        if p["type"] == "bool" and grouped:
            continue
        if p["type"] == "bool":
            style = rng.choice(["if", "if", "ext"])
            if style == "if":
                out += ["if %s:" % n, '    %s.append("%s")' % (var, k)]
            else:
                out.append('%s += ["%s"] if %s else []' % (var, k, n))
        elif p["default"] is None:
            style = rng.choice(["if", "opt", "opt"])
            if style == "opt" and p["type"] == "str":
                out.append('_opt(%s, "%s", %s)' % (var, k, n))
            else:
                out += ["if %s is not None:" % n, "    %s.append(%s)" % (var, _value_expr(p, rng))]
        else:
            style = rng.choice(["if", "ext"])
            if style == "if":
                out += ["if %s != %r:" % (n, p["default"]), "    %s.append(%s)" % (var, _value_expr(p, rng))]
            else:
                out += ["if %s != %r:" % (n, p["default"]), "    %s.extend([%s])" % (var, _value_expr(p, rng))]
    out.append(rng.choice(["return _emit(tokens)", "return _emit(tokens)", "return RENDERER.render(tokens)"]))
    return out


def render_widgets(cat):
    rng = random.Random(SEED + 1)
    lines = HEADER.rstrip("\n").split("\n")
    sec = None
    for w in cat["widgets"]:
        if w["section"] != sec:
            sec = w["section"]
            lines += ["", "", "# " + "=" * 70 + " " + sec]
        lines += ["", ""]
        block = ['@widget("%s", section="%s")' % (w["tag"], w["section"]) if rng.random() < 0.6
                 else '@widget("%s")' % w["tag"]]
        if w["dep"]:
            block.insert(rng.randint(0, 1), "# deprecated: use %s" % w["dep"])
        lines += block
        lines += _sig(w, rng)
        lines += [("    " + x) if x else "" for x in _doc(w, rng)]
        lines += ["    " + x for x in _body(w, rng)]
    return "\n".join(lines) + "\n"


# ------------------------------------------------------------------------------------------ MIGRATION.md


MIGRATION = """# Form widgets: v1 to v2 tokens

`widgets.py` holds the generated v1 widget registry: %(count)d widget functions in %(nsec)d sections
(%(sections)s), each registered by `@widget(...)`. The generator is gone, so the file is migrated in place.
Apply the rules below to every widget function, except where the exceptions table says otherwise. What this
document does not change stays as it is: function names, the element token, the parameters (names, order,
defaults, `*` markers) and every attribute token not named below. A widget still hands its tokens to the
renderer once (`_emit(tokens)` or `RENDERER.render(tokens)`) and returns what the renderer returns; the order
of the attribute tokens after the element token does not matter.

## Terms

- **Name parameter**: the parameter `name`, and any parameter whose name ends in `_name`.
- **Boolean option**: a parameter whose default is `False`; v1 renders it as a bare token (`"required"`) when
  true and leaves it out when false.
- **Value option**: any other optional parameter; v1 renders it as `"key=value"` when it is not `None` and
  differs from its default.
- **Deprecated widget**: a comment line `# deprecated: use NAME` sits among the decorator lines directly above
  its `def`. `NAME` is its replacement, another widget with the same parameters.

## Rule 1: attribute keys

| v1 key | v2 key |
|---|---|
%(renames)s

The key changes in value tokens (`"maxlength=80"` becomes `"max-length=80"`) and boolean tokens alike.

## Rule 2: explicit booleans

Every boolean option is always rendered, as `"<key>=true"` or `"<key>=false"` (key after rule 1).

## Rule 3: explicit defaults

A value option is rendered whenever its value is not `None`, also when it equals the default.

## Rule 4: name parameters are validated

Before rendering, each name parameter is checked in parameter order: a valid value is a `str` matching
`[a-z][a-z0-9_]{0,39}` (the whole string). The first invalid value raises
`ValueError("<widget>: invalid <parameter>: <repr(value)>")` and nothing is rendered.

## Rule 5: deprecated widgets warn and delegate

A deprecated widget emits exactly one warning, category `DeprecationWarning`, message exactly
`<widget>() is deprecated; use <replacement>() instead`, then calls the replacement with the same arguments
(by keyword) and returns what it returns. Its parameter list becomes the replacement's v2 parameter list.

## Rule 6: themes

Every widget gets a last, keyword-only parameter `theme=None` (after a `*` if the widget has none yet). When
`theme` is not `None`, the token `"theme=<theme>"` is rendered too.

## Exceptions

These widgets differ from the rules above in exactly the way stated; every other rule still applies to them.

| Widget | What differs |
|---|---|
%(table)s
"""


def migration_md(cat):
    rows = [(n, "keeps the v1 attribute keys (rule 1 does not apply)") for n in cat["legacy"]]
    rows += [(n, "keeps v1 booleans: a bare `<key>` token when true, nothing when false (rule 2 does not apply)")
             for n in cat["barebools"]]
    rows.sort()
    return MIGRATION % {"count": len(cat["widgets"]), "nsec": len(SECTION_NAMES),
                        "sections": ", ".join(SECTION_NAMES),
                        "renames": "\n".join("| `%s` | `%s` |" % kv for kv in sorted(RENAME.items())),
                        "table": "\n".join("| `%s` | %s |" % kv for kv in rows)}


# ------------------------------------------------------------------------------------------ visible tests


TEST_HEAD = '''"""Examples of the v2 behaviour of widgets.py (see MIGRATION.md).

Run from the workspace root: /usr/bin/python3 -m unittest discover -s tests -v
These cover a few widgets only; the migration applies to every widget function.
"""
import os
import sys
import unittest
import warnings

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

import widgets  # noqa: E402


class Recorder:
    def __init__(self):
        self.calls = []

    def render(self, tokens):
        self.calls.append(list(tokens))
        return len(self.calls)


def call(fn, kwargs):
    rec = Recorder()
    old = widgets.RENDERER
    widgets.RENDERER = rec
    try:
        with warnings.catch_warnings(record=True) as w:
            warnings.simplefilter("always")
            result = getattr(widgets, fn)(**kwargs)
    finally:
        widgets.RENDERER = old
    calls = [[c[0]] + sorted(c[1:]) for c in rec.calls]
    return calls, result, [(x.category.__name__, str(x.message)) for x in w]


class WidgetsV2Test(unittest.TestCase):
    def assertTokens(self, fn, kwargs, expected):
        calls, result, _ = call(fn, kwargs)
        self.assertEqual(calls, [[expected[0]] + sorted(expected[1:])])
        self.assertEqual(result, 1)
'''


def visible_tests(cat):
    byn = by_name(cat)
    special = set(cat["legacy"]) | set(cat["barebools"])
    out = [TEST_HEAD]
    k = 0
    for s in SECTION_NAMES[:3]:
        ws = [w for w in cat["widgets"] if w["section"] == s and not w["dep"] and w["name"] not in special]
        for w in ws[:3]:
            k += 1
            salt = 300 + 13 * k
            out.append("    def test_%s(self):" % w["name"])
            for mode, theme in (("full", "dark" if k % 2 else None), ("defaults", None)):
                kw = kwargs(w, salt, "v", mode)
                if theme:
                    kw["theme"] = theme
                out.append("        self.assertTokens(%r, %r,\n                          %r)" % (
                    w["name"], kw, v2_tokens(w, cat, kw)[1]))
            bad = kwargs(w, salt, "v", "min")
            bad["name"] = ["Bad Name", "", "x" * 41, 7][k % 4]
            r = v2_tokens(w, cat, bad)
            out.append("        with self.assertRaises(ValueError) as cm:")
            out.append("            widgets.%s(**%r)" % (w["name"], bad))
            out.append("        self.assertEqual(str(cm.exception), %r)" % r[1])
            out.append("")
        d = [w for w in cat["widgets"] if w["section"] == s and w["dep"]][0]
        rep = byn[d["dep"]]
        kw = kwargs(rep, 400 + k, "v", "full")
        out.append("    def test_deprecated_%s(self):" % d["name"])
        out.append("        calls, result, warned = call(%r, %r)" % (d["name"], kw))
        out.append("        self.assertEqual(warned, [(\"DeprecationWarning\", %r)])" % dep_message(d))
        t = v2_tokens(rep, cat, kw)[1]
        out.append("        self.assertEqual(calls, [%r])" % ([t[0]] + sorted(t[1:])))
        out.append("")
    out.append("")
    out.append('if __name__ == "__main__":')
    out.append("    unittest.main()")
    return "\n".join(out) + "\n"


def write_workspace(ws):
    cat = build()
    with open(os.path.join(ws, "widgets.py"), "w") as f:
        f.write(render_widgets(cat))
    with open(os.path.join(ws, "MIGRATION.md"), "w") as f:
        f.write(migration_md(cat))
    os.makedirs(os.path.join(ws, "tests"), exist_ok=True)
    with open(os.path.join(ws, "tests", "test_widgets.py"), "w") as f:
        f.write(visible_tests(cat))


if __name__ == "__main__":
    write_workspace(sys.argv[1])
