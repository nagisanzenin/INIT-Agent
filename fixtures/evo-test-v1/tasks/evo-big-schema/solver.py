"""evo-big-schema reference: migrate ./models.py in place as ./MIGRATION.md says.

It reads only the workspace (models.py, the enum and exceptions tables of MIGRATION.md). With the help of the
ast it reads each class's fields (from __init__) and v1 wire keys (from to_dict), then rewrites the class in
the v2 form: renamed unit fields, mapped enum defaults, v2 keys in to_dict, a table-driven from_dict that also
reads v1 payloads, and deprecated classes as warning subclasses placed after their replacement. Python 3.9."""
import ast
import re
import sys

UNITS = {"_mi": ("_km", 1.609344), "_lb": ("_kg", 0.45359237)}

HELPERS = '''

_ENUM_V2 = %(enums)r


def _load(name, data, specs):
    """from_dict for the v2 wire format: specs = (attr, v2 key, v1 key, required, factor, enum)."""
    kwargs = {}
    for attr, key2, key1, required, factor, enum in specs:
        if key2 in data:
            value = data[key2]
        elif key1 in data:
            value = data[key1]
            if factor is not None and value is not None:
                value = round(value * factor, 3)
        elif required:
            raise ValueError("%%s.from_dict: missing %%s" %% (name, attr))
        else:
            continue
        if enum:
            value = _ENUM_V2[attr].get(value, value)
        kwargs[attr] = value
    return kwargs
'''


def doc_tables(md):
    enums = {}
    for f, a, b in re.findall(r"^\| `(status|priority|mode)` \| `(\w+)` \| `(\w+)` \|$", md, re.M):
        enums.setdefault(f, {})[a] = b
    legacy, imperial, keep = set(), set(), set()
    part = md[md.index("## Exceptions"):]
    for name, text in re.findall(r"^\| `(\w+)` \| (.*) \|$", part, re.M):
        if "v1 wire keys" in text:
            legacy.add(name)
        elif "miles and pounds" in text:
            imperial.add(name)
        elif "v1 enum values" in text:
            keep.add(name)
    return enums, legacy, imperial, keep


def wire_keys(fn):
    """attr -> v1 key, from the to_dict function."""
    out = {}

    def attr_of(v):
        if isinstance(v, ast.Attribute) and isinstance(v.value, ast.Name) and v.value.id == "self":
            return v.attr
        return None
    for n in ast.walk(fn):
        if isinstance(n, ast.Dict):
            for k, v in zip(n.keys, n.values):
                if isinstance(k, ast.Constant) and attr_of(v):
                    out[attr_of(v)] = k.value
        elif isinstance(n, ast.Tuple) and len(n.elts) == 2 and isinstance(n.elts[0], ast.Constant) \
                and attr_of(n.elts[1]):
            out[attr_of(n.elts[1])] = n.elts[0].value
        elif isinstance(n, ast.Assign) and isinstance(n.targets[0], ast.Subscript) and attr_of(n.value):
            sl = n.targets[0].slice
            if isinstance(sl, ast.Constant):
                out[attr_of(n.value)] = sl.value
    return out


def main():
    src = open("models.py").read()
    enums, legacy, imperial, keep = doc_tables(open("MIGRATION.md").read())
    lines = src.split("\n")
    tree = ast.parse(src)
    classes = [n for n in tree.body if isinstance(n, ast.ClassDef) and n.name != "Record"]
    spec = {}
    for c in classes:
        init = [n for n in c.body if isinstance(n, ast.FunctionDef) and n.name == "__init__"][0]
        tod = [n for n in c.body if isinstance(n, ast.FunctionDef) and n.name == "to_dict"][0]
        keys = wire_keys(tod)
        args = init.args.args[1:]
        defs = [None] * (len(args) - len(init.args.defaults)) + list(init.args.defaults)
        fields = []
        for a, d in zip(args, defs):
            name, factor = a.arg, None
            if c.name not in imperial:
                for suf, (new, fac) in UNITS.items():
                    if name.endswith(suf):
                        name, factor = name[:-3] + new, fac
            enum = a.arg in enums and c.name not in keep
            default = ast.literal_eval(d) if d is not None else None
            if enum and default is not None:
                default = enums[a.arg].get(default, default)
            key1 = keys[a.arg]
            fields.append({"attr": name, "req": d is None, "default": default, "key1": key1,
                           "key2": key1 if c.name in legacy else name, "factor": factor, "enum": enum})
        dep = None
        i = c.lineno - 2
        while i >= 0 and lines[i].startswith("#"):
            m = re.match(r"^# deprecated: use (\w+)\s*$", lines[i])
            dep = m.group(1) if m else dep
            i -= 1
        body0 = c.body[0]
        doc = lines[body0.lineno - 1:body0.end_lineno] if isinstance(body0, ast.Expr) else []
        spec[c.name] = {"node": c, "fields": fields, "dep": dep, "doc": doc}
    out_blocks = {}
    for name, s in spec.items():
        if s["dep"]:
            continue
        fs = s["fields"]
        params = ", ".join(["self"] + [f["attr"] if f["req"] else "%s=%r" % (f["attr"], f["default"]) for f in fs])
        b = ["class %s(Record):" % name] + s["doc"] + [""]
        b += ["    def __init__(%s):" % params] + ["        self.%s = %s" % (f["attr"], f["attr"]) for f in fs]
        b += ["", "    def to_dict(self):", "        return _compact({"]
        b += ['            "%s": self.%s,' % (f["key2"], f["attr"]) for f in fs]
        b += ["        })", "", "    @classmethod", "    def from_dict(cls, data):",
              "        return cls(**_load(%r, data, (" % name]
        b += ["            (%r, %r, %r, %r, %r, %r)," % (f["attr"], f["key2"], f["key1"], f["req"], f["factor"], f["enum"])
              for f in fs]
        b += ["        )))"]
        out_blocks[name] = b
    for name, s in spec.items():
        if not s["dep"]:
            continue
        rep = spec[s["dep"]]["fields"]
        params = ", ".join(["self"] + [f["attr"] if f["req"] else "%s=%r" % (f["attr"], f["default"]) for f in rep])
        b = ["", "", "# deprecated: use %s" % s["dep"], "class %s(%s):" % (name, s["dep"])] + s["doc"] + [""]
        b += ["    def __init__(%s):" % params,
              '        warnings.warn("%s is deprecated; use %s instead", DeprecationWarning, stacklevel=2)' % (name, s["dep"]),
              "        %s.__init__(self, %s)" % (s["dep"], ", ".join("%s=%s" % (f["attr"], f["attr"]) for f in rep))]
        out_blocks[s["dep"]] = out_blocks[s["dep"]] + b
    # rewrite bottom-up: replace replacement/plain classes, delete deprecated ones (with their comment lines)
    for c in sorted(classes, key=lambda n: n.lineno, reverse=True):
        a, z = c.lineno - 1, c.end_lineno
        if spec[c.name]["dep"]:
            while a > 0 and lines[a - 1].startswith("#"):
                a -= 1
            lines[a:z] = []
        else:
            lines[a:z] = out_blocks[c.name]
    text = "\n".join(lines)
    text = text.replace('"""\n\n\nclass Record:', '"""\nimport warnings\n\n\nclass Record:', 1)
    marker = "\n# " + "=" * 70
    at = text.index(marker)
    text = text[:at] + HELPERS % {"enums": enums} + text[at:]
    text = re.sub(r"\n{4,}", "\n\n\n", text)
    open("models.py", "w").write(text)


if __name__ == "__main__":
    sys.exit(main())
