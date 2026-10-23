"""evo-big-client reference: migrate ./client.py in place as ./MIGRATION.md says.

It reads only the workspace (client.py and the exceptions table of MIGRATION.md) and rewrites each method of
Client with the help of the ast: a new `def` (rules 1 and 4), a prologue (rule-2 and rule-4 checks, aliases
so the v1 body keeps working), the transport call routed through Client._send (rules 1, 4, 5 and 6 on the
wire), and deprecated methods replaced by warn-and-delegate bodies (rule 3). Python 3.9, stdlib only."""
import ast
import re
import sys

MAX_LIMIT = 200
RETRIES = 3

HELPERS = '''

_FIELD_MAP = {
    "weight_lb": ("weight_kg", lambda v: round(v * 0.45359237, 3)),
    "length_in": ("length_cm", lambda v: round(v * 2.54, 2)),
    "temp_f": ("temp_c", lambda v: round((v - 32) * 5 / 9, 1)),
    "qty": ("quantity", lambda v: v),
    "desc": ("description", lambda v: v),
}


def _check_path_param(method, name, value, slash_ok=False):
    if not isinstance(value, str) or value == "" or (not slash_ok and "/" in value):
        raise ValueError("%%s: invalid %%s: %%r" %% (method, name, value))


def _check_limit(method, limit):
    if not isinstance(limit, int) or isinstance(limit, bool) or not 1 <= limit <= %d:
        raise ValueError("%%s: invalid limit: %%r" %% (method, limit))


def _map_body(body):
    out = {}
    for k, v in body.items():
        if k in _FIELD_MAP:
            nk, f = _FIELD_MAP[k]
            out[nk] = f(v)
        else:
            out[k] = v
    return out
''' % MAX_LIMIT

SEND = '''
    def _send(self, how, verb, path, params=None, json=None):
        params = dict(params or {})
        body = None if json is None else dict(json)
        if how.get("rename"):
            for d in (params, body):
                if d is not None and "account_ref" in d:
                    d["account_id"] = d.pop("account_ref")
        if how.get("paging") is not None:
            cursor, limit = how["paging"]
            for k in ("page", "per_page", "page_size"):
                params.pop(k, None)
            if cursor is not None:
                params["cursor"] = cursor
            params["limit"] = limit
        if body is not None and how.get("fields"):
            body = _map_body(body)
        return self._transport.request(verb, path, params=params, json=body, retries=how.get("retries", 0))
'''


def parse_exceptions(md):
    exc = {"legacy": set(), "allow": {}, "keep": set(), "retries": {}, "raw": set()}
    for line in md.splitlines():
        m = re.match(r"^\|\s*`(\w+)`\s*\|\s*(\d)\s*\|(.*)\|\s*$", line)
        if not m:
            continue
        name, rule, text = m.group(1), int(m.group(2)), m.group(3)
        if rule == 1:
            exc["legacy"].add(name)
        elif rule == 2:
            exc["allow"][name] = re.search(r"`(\w+)` may contain", text).group(1)
        elif rule == 4:
            exc["keep"].add(name)
        elif rule == 5:
            exc["retries"][name] = int(re.search(r"retries=(\d+)", text).group(1))
        elif rule == 6:
            exc["raw"].add(name)
    return exc


def endpoint_info(fn):
    for d in fn.decorator_list:
        if isinstance(d, ast.Call) and isinstance(d.func, ast.Name) and d.func.id == "endpoint":
            vals = {}
            for i, a in enumerate(d.args[:2]):
                vals[("verb", "path")[i]] = ast.literal_eval(a)
            for k in d.keywords:
                if k.arg in ("verb", "path", "idempotent"):
                    vals[k.arg] = ast.literal_eval(k.value)
            return vals
    raise SystemExit("no @endpoint on %s" % fn.name)


def deprecated_target(lines, fn):
    top = min([d.lineno for d in fn.decorator_list] + [fn.lineno]) - 1  # 0-based index of the first decorator
    i = top - 1
    while i >= 0 and lines[i].strip().startswith("#"):
        i -= 1
    for line in lines[i + 1:fn.lineno - 1]:
        m = re.match(r"^\s*# deprecated: use (\w+)\s*$", line)
        if m:
            return m.group(1)
    return None


def params_of(fn):
    a = fn.args
    pos = a.args[1:]
    defs = [None] * (len(a.args) - len(a.defaults)) + list(a.defaults)
    out = []
    for i, x in enumerate(a.args):
        if i == 0:
            continue
        d = defs[i]
        out.append([x.arg, False, d is not None, ast.literal_eval(d) if d is not None else None])
    for x, d in zip(a.kwonlyargs, a.kw_defaults):
        out.append([x.arg, True, d is not None, ast.literal_eval(d) if d is not None else None])
    del pos
    return out


def placeholders(path):
    return re.findall(r"\{(\w+)\}", path)


def new_params(name, ps, exc):
    names = [p[0] for p in ps]
    paged = "page" in names and ("per_page" in names or "page_size" in names) and name not in exc["keep"]
    out = []
    for n, kw, hd, d in ps:
        if n == "account_ref" and name not in exc["legacy"]:
            n = "account_id"
        elif paged and n == "page":
            n, d = "cursor", None
        elif paged and n in ("per_page", "page_size"):
            n = "limit"
        out.append([n, kw, hd, d])
    return out, paged


def render_def(name, ps):
    parts = ["self"]
    star = False
    for n, kw, hd, d in ps:
        if kw and not star:
            parts.append("*")
            star = True
        parts.append("%s=%r" % (n, d) if hd else n)
    one = "    def %s(%s):" % (name, ", ".join(parts))
    if len(one) <= 110:
        return [one]
    return ["    def %s(" % name] + ["        %s," % p for p in parts] + ["    ):"]


def main():
    src = open("client.py").read()
    md = open("MIGRATION.md").read()
    exc = parse_exceptions(md)
    lines = src.split("\n")
    tree = ast.parse(src)
    cls = [n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "Client"][0]
    fns = [n for n in cls.body if isinstance(n, ast.FunctionDef) and not n.name.startswith("_")]
    edits = []  # (start0, end0_exclusive, new_lines)
    for fn in fns:
        name = fn.name
        ep = endpoint_info(fn)
        dep = deprecated_target(lines, fn)
        ps = params_of(fn)
        nps, paged = new_params(name, ps, exc)
        body0 = fn.body[0]
        has_doc = isinstance(body0, ast.Expr) and isinstance(getattr(body0, "value", None), ast.Constant) \
            and isinstance(body0.value.value, str)
        sig_start, sig_end = fn.lineno - 1, body0.lineno - 1
        after_doc = body0.end_lineno if has_doc else body0.lineno - 1
        ind = "        "
        if dep:
            call = ", ".join("%s=%s" % (p[0], p[0]) for p in nps)
            new_body = [ind + 'warnings.warn("%s() is deprecated; use %s() instead", DeprecationWarning, '
                        'stacklevel=2)' % (name, dep),
                        ind + "return self.%s(%s)" % (dep, call)]
            edits.append((after_doc, fn.end_lineno, new_body))
            edits.append((sig_start, sig_end, render_def(name, nps)))
            continue
        renamed = any(p[0] == "account_ref" for p in ps) and name not in exc["legacy"]
        pro = []
        if renamed:
            pro.append(ind + "account_ref = account_id")
        for x in placeholders(ep["path"]):
            n = "account_id" if (x == "account_ref" and renamed) else x
            pro.append(ind + "_check_path_param(%r, %r, %s, %r)" % (name, n, n, exc["allow"].get(name) == x))
        size = None
        if paged:
            size = [p[0] for p in ps if p[0] in ("per_page", "page_size")][0]
            pro.append(ind + "_check_limit(%r, limit)" % name)
            pro.append(ind + "page, %s = None, limit" % size)
        retries = exc["retries"].get(name, RETRIES if ep.get("idempotent") else 0)
        how = "dict(retries=%d, rename=%r, fields=%r, paging=%s)" % (
            retries, renamed, name not in exc["raw"], "(cursor, limit)" if paged else "None")
        seg = lines[after_doc:fn.end_lineno]
        hits = [i for i, l in enumerate(seg) if "self._transport.request(" in l]
        if len(hits) != 1:
            raise SystemExit("%s: %d transport calls" % (name, len(hits)))
        seg[hits[0]] = seg[hits[0]].replace("self._transport.request(", "self._send(%s, " % how)
        seg[hits[0]] = seg[hits[0]].replace("self._send(%s, )" % how, "self._send(%s," % how)
        if seg[hits[0]].endswith("self._send(%s, " % how):
            seg[hits[0]] = seg[hits[0]].rstrip()
        edits.append((after_doc, fn.end_lineno, pro + seg))
        edits.append((sig_start, sig_end, render_def(name, nps)))
    for a, b, new in sorted(edits, key=lambda e: e[0], reverse=True):
        lines[a:b] = new
    out = "\n".join(lines)
    out = out.replace("import functools\n", "import functools\nimport warnings\n", 1)
    out = out.replace("\n\nclass Client:", HELPERS + "\n\nclass Client:", 1)
    out = out.replace("        self._transport = transport\n", "        self._transport = transport\n" + SEND, 1)
    open("client.py", "w").write(out)


if __name__ == "__main__":
    sys.exit(main())
