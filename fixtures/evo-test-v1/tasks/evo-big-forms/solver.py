"""evo-big-forms reference: migrate ./widgets.py in place as ./MIGRATION.md says.

It reads only the workspace (widgets.py, the key table and the exceptions of MIGRATION.md). With the help of
the ast it finds each widget's options and the attribute key each one renders, then rewrites the widget: the
v1 head statement (element and name tokens) is kept, name checks come first, the option tokens are rebuilt by
one helper (rules 1-3), the theme parameter is added, and deprecated widgets warn and delegate. Python 3.9."""
import ast
import re
import sys

HELPERS = '''

_RENAME = %(renames)r


def _check_name(widget_name, param, value):
    if not (isinstance(value, str) and re.fullmatch(r"[a-z][a-z0-9_]{0,39}", value)):
        raise ValueError("%%s: invalid %%s: %%r" %% (widget_name, param, value))


def _v2_options(tokens, options, legacy=False, bare=False):
    """options: (v1 key, value, is_boolean) in parameter order."""
    for key, value, boolean in options:
        if not legacy:
            key = _RENAME.get(key, key)
        if boolean:
            if bare:
                if value:
                    tokens.append(key)
            else:
                tokens.append("%%s=%%s" %% (key, "true" if value else "false"))
        elif value is not None:
            tokens.append("%%s=%%s" %% (key, value))
'''


def doc(md):
    renames = dict(re.findall(r"^\| `([a-z-]+)` \| `([a-z-]+)` \|$", md, re.M))
    legacy, bare = set(), set()
    part = md[md.index("## Exceptions"):]
    for name, text in re.findall(r"^\| `(\w+)` \| (.*) \|$", part, re.M):
        if "attribute keys" in text:
            legacy.add(name)
        elif "v1 booleans" in text:
            bare.add(name)
    return renames, legacy, bare


def option_key(body, param):
    """The attribute key the v1 body renders for param: the first string constant of the statement using it."""
    for st in body:
        if not any(isinstance(n, ast.Name) and n.id == param for n in ast.walk(st)):
            continue
        for n in ast.walk(st):
            if isinstance(n, ast.Tuple) and len(n.elts) == 2 and isinstance(n.elts[0], ast.Constant) \
                    and isinstance(n.elts[1], ast.Name) and n.elts[1].id == param:
                return n.elts[0].value
        for part in (st.body if isinstance(st, ast.If) else [st]):
            for n in ast.walk(part):
                if isinstance(n, ast.Constant) and isinstance(n.value, str):
                    return n.value.split("=")[0]
    raise SystemExit("no key for %s" % param)


def main():
    src = open("widgets.py").read()
    renames, legacy, bare = doc(open("MIGRATION.md").read())
    lines = src.split("\n")
    tree = ast.parse(src)
    edits = []
    for fn in tree.body:
        if not isinstance(fn, ast.FunctionDef) or not fn.decorator_list:
            continue
        top = min(d.lineno for d in fn.decorator_list) - 1
        block = lines[top - 1:fn.lineno - 1] if top > 0 else lines[:fn.lineno - 1]
        dep = None
        for line in lines[max(0, top - 3):fn.lineno - 1]:
            m = re.match(r"^# deprecated: use (\w+)\s*$", line)
            dep = m.group(1) if m else dep
        del block
        a = fn.args
        defs = [None] * (len(a.args) - len(a.defaults)) + list(a.defaults)
        params = [(x.arg, False, d) for x, d in zip(a.args, defs)] + \
                 [(x.arg, True, d) for x, d in zip(a.kwonlyargs, a.kw_defaults)]
        parts, star = [], False
        for n, kw, d in params:
            if kw and not star:
                parts.append("*")
                star = True
            parts.append(n if d is None else "%s=%r" % (n, ast.literal_eval(d)))
        if not star:
            parts.append("*")
        parts.append("theme=None")
        sig = ["def %s(" % fn.name] + ["    %s," % p for p in parts] + ["):"]
        body0 = fn.body[0]
        has_doc = isinstance(body0, ast.Expr) and isinstance(body0.value, ast.Constant)
        doc_lines = lines[body0.lineno - 1:body0.end_lineno] if has_doc else []
        stmts = fn.body[1:] if has_doc else fn.body
        names = [n for n, kw, d in params if n == "name" or n.endswith("_name")]
        if dep:
            new = ['    warnings.warn("%s() is deprecated; use %s() instead", DeprecationWarning, stacklevel=2)'
                   % (fn.name, dep),
                   "    return %s(%s)" % (dep, ", ".join("%s=%s" % (n, n) for n, _, _ in params + [("theme", 0, 0)]))]
        else:
            head = lines[stmts[0].lineno - 1:stmts[0].end_lineno]
            opts = []
            for n, kw, d in params:
                if n in names:
                    continue
                boolean = d is not None and ast.literal_eval(d) is False
                opts.append("(%r, %s, %r)" % (option_key(stmts[1:], n), n, boolean))
            new = ["    _check_name(%r, %r, %s)" % (fn.name, n, n) for n in names] + head
            new += ["    _v2_options(tokens, (%s), legacy=%r, bare=%r)" % (
                "".join(o + ", " for o in opts).rstrip(" "), fn.name in legacy, fn.name in bare)]
            new += ["    if theme is not None:", '        tokens.append("theme=%s" % theme)', "    return _emit(tokens)"]
        edits.append((fn.lineno - 1, fn.end_lineno, sig + doc_lines + new))
    for s, e, new in sorted(edits, reverse=True):
        lines[s:e] = new
    text = "\n".join(lines)
    text = text.replace('"""\n\n\nclass _Renderer:', '"""\nimport re\nimport warnings\n\n\nclass _Renderer:', 1)
    at = text.index("\n# " + "=" * 70)
    text = text[:at] + HELPERS % {"renames": renames} + text[at:]
    open("widgets.py", "w").write(text)


if __name__ == "__main__":
    sys.exit(main())
