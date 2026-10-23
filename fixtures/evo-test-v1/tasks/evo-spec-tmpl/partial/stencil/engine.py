"""Stencil template engine (SPEC.md).

render(template, context) -> str; errors raise TemplateError ("<line>:<col>: <message>").
"""

import html
import re

__all__ = ["render", "TemplateError"]

WS = " \t\n\r"
DIGITS = "0123456789"
NAME_START = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz_"
NAME_CHARS = NAME_START + DIGITS
KEYWORDS = ("and", "or", "not", "in", "true", "false", "none")
STATEMENTS = ("if", "elif", "else", "end", "for", "break", "continue", "set", "capture", "macro", "call",
              "verbatim")
LOOPVARS = ("index", "first", "last", "count", "rindex", "depth", "prev", "next")
CMP_OPS = ("==", "!=", "<", ">", "<=", ">=")
TWO_CHAR_OPS = ("??", "==", "!=", "<=", ">=")
VERB_CLOSE = re.compile(r"\{:[-~]?[ \t\r\n]*/verbatim[ \t\r\n]*[-~]?:\}")
FILTER_ARITY = {
    "upper": (0, 0), "lower": (0, 0), "trim": (0, 0), "length": (0, 0), "reverse": (0, 0), "slice": (1, 2),
    "pad": (1, 2), "truncate": (1, 1), "split": (1, 1), "replace": (2, 2), "count": (1, 1), "join": (1, 1),
    "first": (0, 0), "sort": (0, 0), "min": (0, 0), "max": (0, 0), "keys": (0, 0), "items": (0, 0),
    "get": (1, 1), "group": (0, 1), "fixed": (1, 1), "abs": (0, 0), "plus": (1, 1), "plural": (2, 2),
    "safe": (0, 0),
}
MAX_CALLS = 8
MISSING = object()


class TemplateError(Exception):
    """A Stencil error: line and col are 1-based, message is the exact SPEC.md message."""

    def __init__(self, line, col, message):
        Exception.__init__(self, line, col, message)
        self.line = line
        self.col = col
        self.message = message

    def __str__(self):
        return "%d:%d: %s" % (self.line, self.col, self.message)


class _Fail(Exception):
    def __init__(self, pos, message):
        Exception.__init__(self, pos, message)
        self.pos = pos
        self.message = message


class _RErr(Exception):
    def __init__(self, message):
        Exception.__init__(self, message)
        self.message = message


class _Break(Exception):
    pass


class _Continue(Exception):
    pass


# ------------------------------------------------------------------------------------------------ lexing


class _Item(object):
    __slots__ = ("kind", "pos", "end", "text", "lm", "rm", "tag")

    def __init__(self, kind, pos, end, text=None, lm=None, rm=None):
        self.kind = kind
        self.pos = pos
        self.end = end
        self.text = text
        self.lm = lm
        self.rm = rm
        self.tag = None


def _split_markers(raw):
    lm = rm = None
    if raw and raw[0] in "-~":
        lm, raw = raw[0], raw[1:]
    if raw and raw[-1] in "-~":
        rm, raw = raw[-1], raw[:-1]
    return lm, raw, rm


def _lex(src):
    n = len(src)
    i = 0
    while i < n:
        j = i
        while True:
            j = src.find("{", j)
            if j < 0 or j + 1 >= n:
                j = -1
                break
            if src[j + 1] in "=:#":
                break
            j += 1
        if j < 0:
            yield _Item("text", i, n, src[i:])
            break
        if j > i:
            yield _Item("text", i, j, src[i:j])
        opener = src[j + 1]
        if opener == "#":
            p = src.find("#}", j + 2)
            if p < 0:
                raise _Fail(j, "unterminated comment")
            i = p + 2
            continue
        close = "=}" if opener == "=" else ":}"
        k = src.find(close, j + 2)
        if k < 0:
            raise _Fail(j, "unterminated output tag" if opener == "=" else "unterminated statement tag")
        lm, content, rm = _split_markers(src[j + 2:k])
        i = k + 2
        if opener == ":" and content.strip(WS) == "verbatim":
            m = VERB_CLOSE.search(src, i)
            if m is None:
                raise _Fail(j, "unterminated verbatim block")
            yield _Item("vopen", j, i, None, lm, rm)
            yield _Item("raw", i, m.start(), src[i:m.start()])
            closing = m.group(0)
            yield _Item("vclose", m.start(), m.end(), None, None, closing[-3] if closing[-3] in "-~" else None)
            i = m.end()
            continue
        yield _Item("out" if opener == "=" else "stmt", j, i, content, lm, rm)


def _standalone(src, items):
    """Lines holding only a statement tag are removed."""
    for idx, it in enumerate(items):
        if it.kind != "stmt":
            continue
        prev = items[idx - 1] if idx > 0 else None
        nxt = items[idx + 1] if idx + 1 < len(items) else None
        if (prev is not None and prev.kind != "text") or (nxt is not None and nxt.kind != "text"):
            continue
        ptxt = prev.text if prev is not None else ""
        head = ptxt[ptxt.rfind("\n") + 1:]
        if head.strip(" \t") or (prev is not None and "\n" not in ptxt and idx - 1 != 0):
            continue
        ntxt = nxt.text if nxt is not None else ""
        nl = ntxt.find("\n")
        tail = ntxt if nl < 0 else ntxt[:nl]
        if tail.strip(" \t") or (nl < 0 and nxt is not None and idx + 1 != len(items) - 1):
            continue
        if prev is not None:
            prev.text = ptxt[:len(ptxt) - len(head)]
        if nxt is not None:
            nxt.text = ntxt[len(tail) + (1 if nl >= 0 else 0):]


def _standalone_unused(src, items):
    cut_start = {}
    cut_end = {}
    for idx, it in enumerate(items):
        if it.kind not in ("stmt", "comment", "vopen", "vclose") or it.lm or it.rm:
            continue
        ls = src.rfind("\n", 0, it.pos) + 1
        if any(c not in " \t" for c in src[ls:it.pos]):
            continue
        le = src.find("\n", it.end)
        stop = len(src) if le < 0 else le
        if any(c not in " \t" for c in src[it.end:stop]):
            continue
        if it.pos > ls:
            cut_end[idx - 1] = it.pos - ls
        cut = stop - it.end + (0 if le < 0 else 1)
        if cut:
            cut_start[idx + 1] = cut
    for idx, k in cut_start.items():
        items[idx].text = items[idx].text[k:]
    for idx, k in cut_end.items():
        t = items[idx].text
        items[idx].text = t[:len(t) - k]


def _trim(items):
    merged = []
    for it in items:
        if it.kind == "text" and merged and merged[-1].kind == "text":
            merged[-1].text += it.text
        else:
            merged.append(it)
    items[:] = merged
    start = {}
    end = {}
    for idx, it in enumerate(items):
        if it.kind in ("text", "comment"):
            continue
        if it.lm and idx > 0 and items[idx - 1].kind == "text":
            end[idx - 1] = it.lm
        if it.rm and idx + 1 < len(items) and items[idx + 1].kind == "text":
            start[idx + 1] = it.rm
    for idx, it in enumerate(items):
        if it.kind != "text":
            continue
        s = it.text
        if start.get(idx):
            s = s.lstrip()
        if end.get(idx):
            s = s.rstrip()
        it.text = s


# ------------------------------------------------------------------------------------------------ tags


class _Tok(object):
    __slots__ = ("kind", "text", "value", "idx")

    def __init__(self, kind, text, value, idx):
        self.kind = kind
        self.text = text
        self.value = value
        self.idx = idx


def _tokenize(s):
    toks = []
    n = len(s)
    i = 0
    while i < n:
        c = s[i]
        if c in WS:
            i += 1
            continue
        start = i
        if c == '"':
            j = i + 1
            buf = []
            while True:
                if j >= n:
                    raise _RErr("unterminated string")
                ch = s[j]
                if ch == '"':
                    j += 1
                    break
                if ch == "\\":
                    if j + 1 >= n:
                        raise _RErr("unterminated string")
                    e = s[j + 1]
                    if e == "\\" or e == '"':
                        buf.append(e)
                    elif e == "n":
                        buf.append("\n")
                    elif e == "t":
                        buf.append("\t")
                    else:
                        raise _RErr("bad escape '\\%s'" % e)
                    j += 2
                    continue
                buf.append(ch)
                j += 1
            toks.append(_Tok("str", s[start:j], "".join(buf), len(toks)))
            i = j
        elif c in DIGITS or (c == "-" and i + 1 < n and s[i + 1] in DIGITS):
            j = i + 1
            while j < n and s[j] in DIGITS:
                j += 1
            toks.append(_Tok("int", s[start:j], int(s[start:j]), len(toks)))
            i = j
        elif c in NAME_START:
            j = i + 1
            while j < n and s[j] in NAME_CHARS:
                j += 1
            while j + 1 < n and s[j] == "." and s[j + 1] in NAME_CHARS:
                j += 1
                if s[j] in DIGITS:
                    while j < n and s[j] in DIGITS:
                        j += 1
                else:
                    while j < n and s[j] in NAME_CHARS:
                        j += 1
            text = s[start:j]
            toks.append(_Tok("kw" if text in KEYWORDS else "path", text, text, len(toks)))
            i = j
        elif c == "#" and i + 1 < n and s[i + 1] in NAME_START:
            j = i + 2
            while j < n and s[j] in NAME_CHARS:
                j += 1
            toks.append(_Tok("loop", s[start:j], s[start + 1:j], len(toks)))
            i = j
        elif s[i:i + 2] in TWO_CHAR_OPS:
            toks.append(_Tok("op", s[i:i + 2], s[i:i + 2], len(toks)))
            i += 2
        elif c in "|<>=,":
            toks.append(_Tok("op", c, c, len(toks)))
            i += 1
        else:
            raise _RErr("unexpected character '%s'" % c)
    return toks


class _Atom(object):
    __slots__ = ("kind", "text", "value", "idx")

    def __init__(self, tok):
        self.kind = tok.kind
        self.text = tok.text
        self.value = tok.value
        self.idx = tok.idx


class _Filter(object):
    __slots__ = ("name", "args", "idx")

    def __init__(self, name, args, idx):
        self.name = name
        self.args = args
        self.idx = idx


class _Expr(object):
    __slots__ = ("atoms", "filters")

    def __init__(self, atoms, filters):
        self.atoms = atoms
        self.filters = filters


def _is_atom(t):
    return t is not None and (t.kind in ("int", "str", "path", "loop") or
                              (t.kind == "kw" and t.text in ("true", "false", "none")))


def _is_literal(t):
    return t is not None and (t.kind in ("int", "str") or (t.kind == "kw" and t.text in ("true", "false", "none")))


def _is_name(t):
    return t is not None and t.kind == "path" and "." not in t.text


def _literal_value(t):
    if t.kind == "kw":
        return {"true": True, "false": False, "none": None}[t.text]
    return t.value


class _Parser(object):
    def __init__(self, toks):
        self.toks = toks
        self.i = 0
        self.exprs = []
        self.atoms = []

    def peek(self):
        return self.toks[self.i] if self.i < len(self.toks) else None

    def take(self):
        t = self.peek()
        if t is None:
            raise _RErr("unexpected end of tag")
        self.i += 1
        return t

    def bad(self, t):
        raise _RErr("unexpected token '%s'" % t.text)

    def peek_is(self, kind, text):
        t = self.peek()
        return t is not None and t.kind == kind and t.text == text

    def expect(self, kind, text):
        t = self.take()
        if t.kind != kind or t.text != text:
            self.bad(t)

    def name(self):
        t = self.take()
        if not _is_name(t):
            self.bad(t)
        return t

    def done(self):
        t = self.peek()
        if t is not None:
            self.bad(t)

    def atom(self):
        t = self.take()
        if not _is_atom(t):
            self.bad(t)
        a = _Atom(t)
        self.atoms.append(a)
        return a

    def expr(self):
        atoms = [self.atom()]
        while self.peek_is("op", "??"):
            self.i += 1
            atoms.append(self.atom())
        filters = []
        while self.peek_is("op", "|"):
            self.i += 1
            t = self.take()
            if not _is_name(t):
                self.bad(t)
            args = []
            while _is_atom(self.peek()):
                args.append(self.atom())
            filters.append(_Filter(t.text, args, t.idx))
        e = _Expr(atoms, filters)
        self.exprs.append(e)
        return e

    def cond(self):
        parts = [self.and_c()]
        while self.peek_is("kw", "or"):
            self.i += 1
            parts.append(self.and_c())
        return ("or", parts) if len(parts) > 1 else parts[0]

    def and_c(self):
        parts = [self.not_c()]
        while self.peek_is("kw", "and"):
            self.i += 1
            parts.append(self.not_c())
        return ("and", parts) if len(parts) > 1 else parts[0]

    def not_c(self):
        if self.peek_is("kw", "not"):
            self.i += 1
            return ("not", self.not_c())
        left = self.expr()
        t = self.peek()
        if t is not None and ((t.kind == "op" and t.text in CMP_OPS) or (t.kind == "kw" and t.text == "in")):
            self.i += 1
            return ("cmp", t.text, left, self.expr())
        return ("val", left)


def _parse_tag(it):
    toks = _tokenize(it.text)
    if not toks:
        raise _RErr("empty tag")
    p = _Parser(toks)
    problems = []
    if it.kind == "out":
        tag = ("out", p.expr())
    else:
        first = p.take()
        kw = first.text
        if first.kind != "path" or kw not in STATEMENTS:
            raise _RErr("unknown statement '%s'" % first.text)
        if kw in ("if", "elif"):
            tag = (kw, p.cond())
        elif kw in ("else", "end", "verbatim", "break", "continue"):
            tag = (kw,)
        elif kw == "for":
            names = [p.name().text]
            if p.peek_is("op", ","):
                p.i += 1
                names.append(p.name().text)
            p.expect("kw", "in")
            tag = ("for", names, p.expr())
        elif kw == "set":
            name = p.name().text
            p.expect("op", "=")
            tag = ("set", name, p.expr())
        elif kw == "capture":
            tag = ("capture", p.name().text)
        elif kw == "call":
            name = p.name().text
            args = []
            while _is_atom(p.peek()):
                args.append(p.atom())
            tag = ("call", name, args)
        else:
            name = p.name().text
            params = []
            while p.peek() is not None:
                t = p.name()
                default = MISSING
                if p.peek_is("op", "="):
                    p.i += 1
                    d = p.take()
                    if not _is_literal(d):
                        p.bad(d)
                    default = _literal_value(d)
                params.append((t.text, default, t.idx))
            seen = set()
            had_default = False
            for pname, default, idx in params:
                if pname in seen:
                    problems.append((idx, "duplicate parameter '%s'" % pname))
                elif default is MISSING and had_default:
                    problems.append((idx, "parameter '%s' needs a default" % pname))
                seen.add(pname)
                if default is not MISSING:
                    had_default = True
            tag = ("macro", name, [(n, d) for n, d, _ in params])
    p.done()
    for a in p.atoms:
        if a.kind == "loop" and a.value not in LOOPVARS:
            problems.append((a.idx, "unknown loop variable '%s'" % a.text))
    for e in p.exprs:
        for f in e.filters:
            if f.name not in FILTER_ARITY:
                problems.append((f.idx, "unknown filter '%s'" % f.name))
            elif not FILTER_ARITY[f.name][0] <= len(f.args) <= FILTER_ARITY[f.name][1]:
                problems.append((f.idx, "wrong number of arguments for filter '%s'" % f.name))
    if problems:
        problems.sort(key=lambda x: x[0])
        raise _RErr(problems[0][1])
    return tag


# ------------------------------------------------------------------------------------------------ structure


class _Block(object):
    def __init__(self, kind, pos, tag):
        self.kind = kind
        self.pos = pos
        self.tag = tag
        self.branches = []  # if: [(pos, cond, body)]
        self.body = []
        self.else_body = None


def _check_structure(items):
    """Structure check of the items seen so far (an unclosed block is fine until the end)."""
    depth = []
    for it in items:
        kw = it.tag[0] if it.tag else None
        if kw in ("if", "for", "capture", "macro"):
            depth.append([kw, False])
        elif kw == "elif":
            if not depth or depth[-1][0] != "if" or depth[-1][1]:
                raise _Fail(it.pos, "unexpected 'elif'")
        elif kw == "else":
            if not depth or depth[-1][0] not in ("if", "for") or depth[-1][1]:
                raise _Fail(it.pos, "unexpected 'else'")
            depth[-1][1] = True
        elif kw == "end":
            if not depth:
                raise _Fail(it.pos, "unexpected 'end'")
            depth.pop()


def _build(items):
    root = _Block("root", 0, None)
    stack = [root]
    macros = {}
    calls = []

    def cur_body():
        b = stack[-1]
        if b.else_body is not None:
            return b.else_body
        if b.kind == "if":
            return b.branches[-1][2]
        return b.body

    for it in items:
        if it.kind in ("text", "raw"):
            if it.text:
                cur_body().append(("text", it.text))
            continue
        if it.kind in ("comment", "vopen", "vclose"):
            continue
        tag = it.tag
        kw = tag[0]
        if kw == "out":
            cur_body().append(("out", it.pos, tag[1]))
        elif kw == "set":
            cur_body().append(("set", it.pos, tag[1], tag[2]))
        elif kw == "call":
            cur_body().append(("call", it.pos, tag[1], tag[2]))
            calls.append((it.pos, tag[1]))
        elif kw in ("break", "continue"):
            if not any(b.kind == "for" for b in stack):
                raise _Fail(it.pos, "unexpected '%s'" % kw)
            cur_body().append((kw, it.pos))
        elif kw == "if":
            b = _Block("if", it.pos, tag)
            b.branches.append((it.pos, tag[1], []))
            cur_body().append(("block", b))
            stack.append(b)
        elif kw in ("for", "capture"):
            b = _Block(kw, it.pos, tag)
            cur_body().append(("block", b))
            stack.append(b)
        elif kw == "macro":
            if len(stack) != 1:
                raise _Fail(it.pos, "macro must be at the top level")
            if tag[1] in macros:
                raise _Fail(it.pos, "macro '%s' is already defined" % tag[1])
            b = _Block("macro", it.pos, tag)
            macros[tag[1]] = b
            stack.append(b)
        elif kw == "elif":
            b = stack[-1]
            if b.kind != "if" or b.else_body is not None:
                raise _Fail(it.pos, "unexpected 'elif'")
            b.branches.append((it.pos, tag[1], []))
        elif kw == "else":
            b = stack[-1]
            if b.kind not in ("if", "for") or b.else_body is not None:
                raise _Fail(it.pos, "unexpected 'else'")
            b.else_body = []
        elif kw == "end":
            if len(stack) == 1:
                raise _Fail(it.pos, "unexpected 'end'")
            stack.pop()
    if len(stack) > 1:
        b = stack[-1]
        raise _Fail(b.pos, "unclosed '%s'" % b.kind)
    return root.body, macros


# ------------------------------------------------------------------------------------------------ values


def _tname(v):
    if isinstance(v, bool):
        return "bool"
    if v is None:
        return "none"
    if isinstance(v, int):
        return "int"
    if isinstance(v, str):
        return "string"
    if isinstance(v, list):
        return "list"
    if isinstance(v, dict):
        return "map"
    return type(v).__name__


def _fmt(v):
    if isinstance(v, bool):
        return "true" if v else "false"
    if v is None:
        return ""
    if isinstance(v, (int, str)):
        return str(v)
    if isinstance(v, list):
        return ", ".join(_fmt(x) for x in v)
    if isinstance(v, dict):
        raise _RErr("cannot output a map")
    return str(v)


def _escape(s):
    return html.escape(s)


def _truthy(v):
    if v is None or v is MISSING:
        return False
    return bool(v)


def _eq(a, b):
    return a == b


def _is_int(v):
    return isinstance(v, int) and not isinstance(v, bool)


def _ascii_case(s, lo, hi, delta):
    return "".join(chr(ord(c) + delta) if lo <= c <= hi else c for c in s)


def _same_kind_items(v):
    return all(_is_int(x) for x in v) or all(isinstance(x, str) for x in v)


INPUTS = {
    "upper": ("string",), "lower": ("string",), "trim": ("string",), "truncate": ("string",),
    "split": ("string",), "replace": ("string",), "length": ("string", "list", "map"),
    "reverse": ("string", "list"), "slice": ("string", "list"), "count": ("string", "list"),
    "pad": ("string", "int"), "join": ("list",), "first": ("list",), "sort": ("list",), "min": ("list",),
    "max": ("list",), "keys": ("map",), "items": ("map",), "get": ("map", "list"), "group": ("int",),
    "fixed": ("int",), "abs": ("int",), "plus": ("int", "string", "list"), "plural": ("int",),
}


def _apply(name, v, args):
    if name == "safe":
        return v
    t = _tname(v)
    if t not in INPUTS[name]:
        raise _RErr("filter '%s' cannot take %s" % (name, t))

    def badarg(n):
        raise _RErr("bad argument %d for filter '%s'" % (n, name))

    def need(n, ok):
        if not ok:
            badarg(n)
        return args[n - 1]

    if name == "upper":
        return v.upper()
    if name == "lower":
        return v.lower()
    if name == "trim":
        return v.strip()
    if name == "length":
        return len(v)
    if name == "reverse":
        return v[::-1]
    if name == "slice":
        a = need(1, _is_int(args[0]))
        b = len(v)
        if len(args) > 1:
            b = need(2, _is_int(args[1]))
        return v[a:b]
    if name == "pad":
        w = need(1, _is_int(args[0]))
        ch = " "
        if len(args) > 1:
            ch = need(2, isinstance(args[1], str) and len(args[1]) == 1)
        s = str(v)
        if w >= 0:
            return ch * (w - len(s)) + s
        return s + ch * (-w - len(s))
    if name == "truncate":
        n = need(1, _is_int(args[0]) and args[0] >= 0)
        return v[:max(n - 3, 0)] + "..." if len(v) > n else v
    if name == "split":
        sep = need(1, isinstance(args[0], str) and args[0] != "")
        return v.split(sep)
    if name == "replace":
        old = need(1, isinstance(args[0], str) and args[0] != "")
        new = need(2, isinstance(args[1], str))
        return v.replace(old, new)
    if name == "count":
        if t == "string":
            x = need(1, isinstance(args[0], str) and args[0] != "")
            return v.count(x)
        return sum(1 for item in v if _eq(item, args[0]))
    if name == "join":
        sep = need(1, isinstance(args[0], str))
        return sep.join(_fmt(x) for x in v)
    if name == "first":
        return v[0] if v else None
    if name == "sort":
        try:
            return sorted(v)
        except TypeError:
            raise _RErr("filter 'sort' cannot sort these items")
    if name in ("min", "max"):
        if not v:
            return None
        try:
            return min(v) if name == "min" else max(v)
        except TypeError:
            raise _RErr("filter '%s' cannot compare these items" % name)
    if name == "keys":
        return sorted(v)
    if name == "items":
        return [[k, v[k]] for k in sorted(v)]
    if name == "get":
        if t == "map":
            k = need(1, isinstance(args[0], str))
            return v.get(k)
        k = need(1, _is_int(args[0]))
        return v[k] if 0 <= k < len(v) else None
    if name == "group":
        sep = ","
        if args:
            sep = need(1, isinstance(args[0], str))
        s = str(abs(v))
        parts = []
        while len(s) > 3:
            parts.insert(0, s[-3:])
            s = s[:-3]
        parts.insert(0, s)
        return ("-" if v < 0 else "") + sep.join(parts)
    if name == "fixed":
        n = need(1, _is_int(args[0]) and 0 <= args[0] <= 6)
        if n == 0:
            return str(v)
        return "%d.%0*d" % (v // 10 ** n, n, v % 10 ** n)
    if name == "abs":
        return abs(v)
    if name == "plus":
        try:
            return v + args[0]
        except TypeError:
            badarg(1)
    one = need(1, isinstance(args[0], str))  # plural
    many = need(2, isinstance(args[1], str))
    return one if v == 1 else many


# ------------------------------------------------------------------------------------------------ rendering


class _Renderer(object):
    def __init__(self, context, macros):
        self.context = context
        self.macros = macros
        self.scopes = [{}]
        self.meta = []
        self.out = []
        self.calls = 0

    def lookup(self, text):
        if text.startswith("#"):
            if not self.meta:
                return MISSING
            idx, rows = self.meta[-1]
            count = len(rows)
            name = text[1:]
            if name == "index":
                return idx
            if name == "count":
                return count
            if name == "first":
                return idx == 1
            if name == "last":
                return idx == count
            if name == "rindex":
                return count - idx + 1
            if name == "depth":
                return len(self.meta)
            if name == "prev":
                return rows[idx - 2][0] if idx > 1 else None
            return rows[idx][0] if idx < count else None
        segs = text.split(".")
        head = segs[0]
        for scope in reversed(self.scopes):
            if head in scope:
                v = scope[head]
                break
        else:
            if head not in self.context:
                return MISSING
            v = self.context[head]
        for s in segs[1:]:
            if isinstance(v, dict):
                if s not in v:
                    return MISSING
                v = v[s]
            elif isinstance(v, list) and s[0] in DIGITS:
                k = int(s)
                if k >= len(v):
                    return MISSING
                v = v[k]
            else:
                return MISSING
        return v

    def raw_atom(self, a):
        if a.kind in ("int", "str"):
            return a.value
        if a.kind == "kw":
            return {"true": True, "false": False, "none": None}[a.text]
        return self.lookup(a.text)

    def atom(self, a, strict):
        v = self.raw_atom(a)
        if v is MISSING:
            if strict:
                raise _RErr("undefined variable '%s'" % a.text)
            return None
        return v

    def expr(self, e, strict):
        v = None
        last = len(e.atoms) - 1
        for k, a in enumerate(e.atoms):
            v = self.raw_atom(a)
            if v is not MISSING and v is not None:
                break
            if k == last:
                v = self.atom(a, strict)
        for f in e.filters:
            if f.name != "safe" and _tname(v) not in INPUTS[f.name]:
                raise _RErr("filter '%s' cannot take %s" % (f.name, _tname(v)))
            args = [self.atom(a, strict) for a in f.args]
            v = _apply(f.name, v, args)
        return v

    def cond(self, c):
        kind = c[0]
        if kind == "or":
            return any(self.cond(x) for x in c[1])
        if kind == "and":
            return all(self.cond(x) for x in c[1])
        if kind == "not":
            return not self.cond(c[1])
        if kind == "val":
            return _truthy(self.expr(c[1], False))
        op = c[1]
        a = self.expr(c[2], False)
        b = self.expr(c[3], False)
        if op == "==":
            return _eq(a, b)
        if op == "!=":
            return not _eq(a, b)
        ta, tb = _tname(a), _tname(b)
        if op == "in":
            if b is None:
                return False
            try:
                return a in b
            except TypeError:
                raise _RErr("cannot test membership in %s" % tb)
        if not (ta == tb and ta in ("int", "string")):
            raise _RErr("cannot compare %s and %s" % (ta, tb))
        if op == "<":
            return a < b
        if op == ">":
            return a > b
        if op == "<=":
            return a <= b
        return a >= b

    def run(self, body):
        for node in body:
            kind = node[0]
            if kind == "text":
                self.out.append(node[1])
            elif kind == "out":
                e = node[2]
                try:
                    s = _fmt(self.expr(e, True))
                except _RErr as x:
                    raise _Fail(node[1], x.message)
                if not any(f.name == "safe" for f in e.filters):
                    s = _escape(s)
                self.out.append(s)
            elif kind == "set":
                try:
                    v = self.expr(node[3], True)
                except _RErr as x:
                    raise _Fail(node[1], x.message)
                self.scopes[0][node[2]] = v
            elif kind == "call":
                self.run_call(node)
            elif kind == "break":
                raise _Break()
            elif kind == "continue":
                raise _Continue()
            else:
                b = node[1]
                if b.kind == "if":
                    self.run_if(b)
                elif b.kind == "for":
                    self.run_for(b)
                else:
                    saved = self.out
                    self.out = []
                    try:
                        self.run(b.body)
                        text = "".join(self.out)
                    finally:
                        self.out = saved
                    self.scopes[-1][b.tag[1]] = text

    def run_call(self, node):
        pos, name, arg_atoms = node[1], node[2], node[3]
        if name not in self.macros:
            raise _Fail(pos, "unknown macro '%s'" % name)
        m = self.macros[name]
        params = m.tag[2]
        try:
            args = [self.atom(a, True) for a in arg_atoms]
            required = sum(1 for _, d in params if d is MISSING)
            if len(args) > len(params):
                raise _RErr("macro '%s' takes at most %d arguments, got %d" % (name, len(params), len(args)))
            if len(args) < required:
                raise _RErr("macro '%s' needs at least %d arguments, got %d" % (name, required, len(args)))
            if self.calls > MAX_CALLS:
                raise _RErr("macro calls nested too deeply")
        except _RErr as x:
            raise _Fail(pos, x.message)
        scope = {}
        for k, (pname, default) in enumerate(params):
            scope[pname] = args[k] if k < len(args) else default
        self.scopes.append(scope)
        self.calls += 1
        try:
            self.run(m.body)
        finally:
            self.calls -= 1
            self.scopes.pop()

    def run_if(self, b):
        for pos, c, body in b.branches:
            try:
                ok = self.cond(c)
            except _RErr as x:
                raise _Fail(pos, x.message)
            if ok:
                self.run(body)
                return
        if b.else_body is not None:
            self.run(b.else_body)

    def run_for(self, b):
        names = b.tag[1]
        try:
            v = self.expr(b.tag[2], True)
            t = _tname(v)
            if v is None:
                rows = []
            elif t in ("list", "string"):
                if len(names) == 2:
                    raise _RErr("two loop variables need a map")
                rows = [(x,) for x in v]
            elif t == "map":
                rows = list(v.items()) if len(names) == 2 else [(k,) for k in v]
            else:
                raise _RErr("cannot iterate over %s" % t)
        except _RErr as x:
            raise _Fail(b.pos, x.message)
        if not rows:
            if b.else_body is not None:
                self.run(b.else_body)
            return
        for i, row in enumerate(rows):
            self.scopes.append(dict(zip(names, row)))
            self.meta.append((i + 1, rows))
            try:
                self.run(b.body)
            except _Continue:
                pass
            except _Break:
                break
            finally:
                self.meta.pop()
                self.scopes.pop()


def _position(src, pos):
    line = src.count("\n", 0, pos) + 1
    col = pos - (src.rfind("\n", 0, pos) + 1) + 1
    return line, col


def render(template, context):
    """Render `template` (str) with `context` (dict with str keys); see SPEC.md."""
    try:
        items = []
        for it in _lex(template):
            if it.kind in ("out", "stmt"):
                try:
                    it.tag = _parse_tag(it)
                except _RErr as x:
                    raise _Fail(it.pos, x.message)
            items.append(it)
            _check_structure(items)
        _trim(items)
        _standalone(template, items)
        body, macros = _build(items)
        r = _Renderer(context, macros)
        r.run(body)
        return "".join(r.out)
    except _Fail as f:
        line, col = _position(template, f.pos)
        raise TemplateError(line, col, f.message)
