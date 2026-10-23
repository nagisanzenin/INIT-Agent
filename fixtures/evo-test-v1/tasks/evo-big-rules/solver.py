"""evo-big-rules reference: migrate ./rules.py in place as ./MIGRATION.md says.

It reads only the workspace (rules.py, and the tables of MIGRATION.md). With the help of the ast it renames
each plain rule's v1 body to _v1_<name>(record, ctx) (TODAY/STRICT read from ctx) behind a public
<name>(record, ctx) that turns the v1 finding into the v2 one; composed rules get ctx threaded into their
sub-rule calls; retired and merged rules become forwarding shims; their ids leave RULES. Python 3.9, stdlib."""
import ast
import re
import sys

HELPERS = '''

_CODE_MAP = %(codes)r
_KEEP_CODE = %(keep)r
_PINNED = %(pinned)r


def _finish(rule_id, category, finding):
    if finding is None:
        return None
    code, severity, message = finding
    if rule_id not in _KEEP_CODE:
        code = _CODE_MAP.get((code, category)) or _CODE_MAP[(code, "*")]
    if rule_id in _PINNED:
        severity = _PINNED[rule_id]
    elif severity == "error" and category in ("money", "identity"):
        severity = "blocker"
    elif severity == "warning" and category == "style":
        severity = "info"
    return (rule_id, code, severity, message)
'''


def tables(md):
    def rows(after):
        part = md[md.index(after):]
        part = part[part.index("|---"):]
        out = []
        for line in part.split("\n")[1:]:
            if not line.startswith("|"):
                break
            out.append([c.strip() for c in line.strip("|").split("|")])
        return out

    def tick(c):
        return re.findall(r"`([^`]*)`", c)
    codes = {}
    for a, k, b in rows("## Rule 3"):
        codes[(tick(a)[0], tick(k)[0] if tick(k) else "*")] = tick(b)[0]
    retired = dict((tick(a)[0], tick(b)[0]) for a, b in rows("| Retired |"))
    merged = dict((tick(a)[0], tick(b)[0]) for a, b in rows("| Merged |"))
    keep, pinned, strict = [], {}, []
    for a, b in rows("## Exceptions"):
        rid = tick(a)[0]
        if "keeps its v1 code" in b:
            keep.append(rid)
        elif "severity is always" in b:
            pinned[rid] = tick(b)[0]
        elif "strict mode on" in b:
            strict.append(rid)
    return codes, retired, merged, keep, pinned, strict


def registry(tree):
    reg = {}
    for n in tree.body:
        d = None
        if isinstance(n, ast.Assign) and isinstance(n.value, ast.Dict):
            d = n.value
        elif isinstance(n, ast.Expr) and isinstance(n.value, ast.Call) and n.value.args \
                and isinstance(n.value.args[0], ast.Dict):
            d = n.value.args[0]
        if d is None:
            continue
        for k, v in zip(d.keys, d.values):
            reg[k.value] = (v.elts[0].id, v.elts[1].value, (k.lineno, v.end_lineno))
    return reg


def main():
    src = open("rules.py").read()
    codes, retired, merged, keep, pinned, strict = tables(open("MIGRATION.md").read())
    lines = src.split("\n")
    tree = ast.parse(src)
    reg = registry(tree)
    fn_id = dict((fn, (rid, cat)) for rid, (fn, cat, _) in reg.items())
    id_fn = dict((rid, fn) for rid, (fn, cat, _) in reg.items())
    survivors = dict((a, b) for b, a in merged.items())
    edits = []  # (line index, kind, payload); applied bottom-up
    first_rule = None
    # helpers that read TODAY / STRICT get a ctx parameter; their callers pass ctx on
    helpers = set()
    runners = set()
    for n in tree.body:
        if isinstance(n, ast.FunctionDef) and n.name not in fn_id and any(
                isinstance(c, ast.Call) and len(c.args) == 1 and isinstance(c.args[0], ast.Name)
                and c.args[0].id == "record" for c in ast.walk(n)):
            runners.add(n.name)  # a shared runner of sub-rules: takes ctx after record, passes it on
            for c in ast.walk(n):
                if isinstance(c, ast.Call) and len(c.args) == 1 and isinstance(c.args[0], ast.Name) \
                        and c.args[0].id == "record":
                    edits.append((c.lineno - 1, "sub", (c.args[0].col_offset, c.args[0].end_col_offset, "record, ctx")))
            first = n.args.args[0]
            edits.append((first.lineno - 1, "sub", (first.col_offset, first.end_col_offset, "record, ctx")))
    for n in tree.body:
        if isinstance(n, ast.FunctionDef) and n.name not in fn_id and any(
                isinstance(x, ast.Name) and x.id in ("TODAY", "STRICT") for x in ast.walk(n)):
            helpers.add(n.name)
            a = n.args.args
            edits.append((n.lineno - 1, "sub", (0, len(lines[n.lineno - 1]),
                                                lines[n.lineno - 1].replace("(%s):" % ", ".join(x.arg for x in a),
                                                                            "(%s):" % ", ".join([x.arg for x in a] + ["ctx"])))))
            for x in ast.walk(n):
                if isinstance(x, ast.Name) and x.id in ("TODAY", "STRICT"):
                    edits.append((x.lineno - 1, "sub", (x.col_offset, x.end_col_offset,
                                                        'ctx["%s"]' % ("today" if x.id == "TODAY" else "strict"))))
    for n in tree.body:
        if not isinstance(n, ast.FunctionDef) or n.name not in fn_id:
            continue
        rid, cat = fn_id[n.name]
        first_rule = n if first_rule is None else first_rule
        calls = [c for c in ast.walk(n) if isinstance(c, ast.Call) and c.args
                 and isinstance(c.args[0], ast.Name) and c.args[0].id == "record"
                 and (len(c.args) == 1 or (isinstance(c.func, ast.Name) and c.func.id in runners))]
        subs = []  # (line0, col, end_col, text)
        for node in ast.walk(n):
            if isinstance(node, ast.Call) and isinstance(node.func, ast.Name) and node.func.id in helpers:
                end = node.end_col_offset - 1
                subs.append((node.end_lineno - 1, end, end, ", ctx" if node.args else "ctx"))
            if isinstance(node, ast.Name) and node.id == "TODAY":
                subs.append((node.lineno - 1, node.col_offset, node.end_col_offset, 'ctx["today"]'))
            elif isinstance(node, ast.Name) and node.id == "STRICT":
                subs.append((node.lineno - 1, node.col_offset, node.end_col_offset, 'ctx["strict"]'))
        if calls:  # composed: thread ctx, and look retired / merged ids up under their replacement
            arg = "dict(ctx, strict=True)" if rid in strict else "ctx"
            for c in calls:
                a = c.args[0]
                subs.append((a.lineno - 1, a.col_offset, a.end_col_offset, "record, " + arg))
            for node in ast.walk(n):
                if isinstance(node, ast.Constant) and isinstance(node.value, str) and node.value in reg:
                    new = retired.get(node.value) or merged.get(node.value)
                    if new:
                        subs.append((node.lineno - 1, node.col_offset, node.end_col_offset, '"%s"' % new))
            edits.append((n.lineno - 1, "def", (n.name, n.name)))
        else:
            edits.append((n.lineno - 1, "def", (n.name, "_v1_" + n.name)))
            if rid in retired or rid in merged:
                target = id_fn[retired.get(rid) or merged.get(rid)]
                body = ["", "", "def %s(record, ctx):" % n.name, "    return %s(record, ctx)" % target]
            elif rid in survivors:
                other = id_fn[survivors[rid]]
                body = ["", "", "def %s(record, ctx):" % n.name,
                        "    return (_finish(%r, %r, _v1_%s(record, ctx))" % (rid, cat, n.name),
                        "            or _finish(%r, %r, _v1_%s(record, ctx)))" % (rid, cat, other)]
            else:
                body = ["", "", "def %s(record, ctx):" % n.name,
                        "    return _finish(%r, %r, _v1_%s(record, ctx))" % (rid, cat, n.name)]
            edits.append((n.end_lineno, "insert", body))
        for s in subs:
            edits.append((s[0], "sub", s[1:]))
    # registry: drop retired and merged ids
    for rid in list(retired) + list(merged):
        a, b = reg[rid][2]
        for i in range(a - 1, b):
            edits.append((i, "drop", None))
    # apply: substitutions right to left within a line, lines bottom-up
    order = {"insert": 0, "drop": 1, "sub": 2, "def": 3}
    edits.sort(key=lambda e: (e[0], order[e[1]], e[2][0] if e[1] == "sub" else 0), reverse=True)
    for idx, kind, p in edits:
        if kind == "insert":
            lines[idx:idx] = p
        elif kind == "drop":
            del lines[idx]
        elif kind == "sub":
            col, end, text = p
            lines[idx] = lines[idx][:col] + text + lines[idx][end:]
        else:
            old, new = p
            lines[idx] = re.sub(r"^def %s\(record(, ctx)?\):" % old, "def %s(record, ctx):" % new, lines[idx])
            assert lines[idx].startswith("def %s(record, ctx):" % new), lines[idx]
    helpers = HELPERS % {"codes": codes, "keep": sorted(keep), "pinned": pinned}
    at = first_rule.lineno - 1
    while at > 0 and (lines[at - 1].startswith("#") or not lines[at - 1].strip()):
        at -= 1
    lines[at:at] = helpers.rstrip("\n").split("\n")
    open("rules.py", "w").write("\n".join(lines))


if __name__ == "__main__":
    sys.exit(main())
