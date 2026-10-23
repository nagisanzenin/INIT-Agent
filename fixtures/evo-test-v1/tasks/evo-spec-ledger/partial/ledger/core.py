"""PLT ledger: normalize and validate Plain Ledger Text (SPEC.md, version 4)."""
import decimal
import re
from decimal import Decimal

__all__ = ["LedgerError", "Result", "normalize_date", "normalize_amount", "normalize_account", "process"]

_CTX = decimal.Context(prec=200, rounding=decimal.ROUND_HALF_UP)
_LIMIT = Decimal(10) ** 15
_MAX_ERRORS = 20


class LedgerError(Exception):
    """An invalid line; str(exc) is the exact message of SPEC.md (without the line prefix)."""


class Result(object):
    """The outcome of process(): the normal-form text and the error strings."""

    def __init__(self, output, errors):
        self.output = output
        self.errors = errors

    def __repr__(self):
        return "Result(output=%r, errors=%r)" % (self.output, self.errors)


def _tokens(s):
    return [t for t in re.split(r"[ \t]+", s) if t]


# ------------------------------------------------------------------------------------------- dates

_MONTHS = {m: i + 1 for i, m in enumerate("jan feb mar apr may jun jul aug sep oct nov dec".split())}
_FULL = ((re.compile(r"([0-9]{4})-([0-9]{2})-([0-9]{2})"), "ymd"),
         (re.compile(r"([0-9]{4})/([0-9]{2})/([0-9]{2})"), "ymd"),
         (re.compile(r"([0-9]{2})\.([0-9]{2})\.([0-9]{4})"), "dmy"),
         (re.compile(r"([0-9]{4})([0-9]{2})([0-9]{2})"), "ymd"))
_MON = re.compile(r"([0-9]{1,2})-([A-Za-z]{3})-([0-9]{4})")
_SHORT = ((re.compile(r"([0-9]{2})-([0-9]{2})"), "md"), (re.compile(r"([0-9]{2})/([0-9]{2})"), "md"))
_SHORT_MON = re.compile(r"([0-9]{1,2})-([A-Za-z]{3})")


def _leap(y):
    return y % 4 == 0 and (y % 100 != 0 or y % 400 == 0)


def _date(tok, year=None):
    ymd = None
    for rx, order in _FULL:
        m = rx.fullmatch(tok)
        if m:
            g = dict(zip(order, (int(x) for x in m.groups())))
            ymd = (g["y"], g["m"], g["d"])
            break
    if ymd is None:
        m = _MON.fullmatch(tok)
        if m and m.group(2).lower() in _MONTHS:
            ymd = (int(m.group(3)), _MONTHS[m.group(2).lower()], int(m.group(1)))
    if ymd is None and year is not None:
        for rx, order in _SHORT:
            m = rx.fullmatch(tok)
            if m:
                ymd = (year, int(m.group(1)), int(m.group(2)))
                break
        if ymd is None:
            m = _SHORT_MON.fullmatch(tok)
            if m and m.group(2).lower() in _MONTHS:
                ymd = (year, _MONTHS[m.group(2).lower()], int(m.group(1)))
    if ymd is None:
        raise LedgerError("bad date '%s'" % tok)
    y, mo, d = ymd
    if not 1900 <= y <= 2199:
        raise LedgerError("date out of range '%s'" % tok)
    days = [31, 29 if _leap(y) else 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    if not 1 <= mo <= 12 or not 1 <= d <= days[mo - 1]:
        raise LedgerError("invalid date '%s'" % tok)
    return "%04d-%02d-%02d" % (y, mo, d)


def normalize_date(text):
    return _date(text.strip(" \t"))


# ------------------------------------------------------------------------------------ currencies

_BUILTIN = (("USD", 2, ("US$", "DOLLAR")), ("EUR", 2, ("EURO",)), ("GBP", 2, ("STG",)), ("JPY", 0, ("YEN",)),
            ("VND", 0, ("DONG",)), ("KWD", 3, ()), ("CLF", 4, ("UF",)), ("BTC", 8, ("XBT",)))


class _Currencies(object):
    def __init__(self):
        self.names = {}
        self.decimals = {}
        for code, dec, aliases in _BUILTIN:
            self.add(code, dec, aliases)

    def add(self, code, dec, aliases):
        self.decimals[code] = dec
        for name in (code,) + tuple(aliases):
            self.names[name.lower()] = code


_NUM = re.compile(r"([0-9,]*)(?:\.([0-9]+))?")
_GROUPED = re.compile(r"[0-9]{1,3}(?:,[0-9]{3})+")


def _number(tok):
    neg = False
    body = tok
    if len(tok) >= 2 and tok[0] == "(" and tok[-1] == ")":
        body = tok[1:-1]
        if body.startswith("-"):
            raise LedgerError("bad number '%s'" % tok)
        neg = True
    elif tok.startswith("-"):
        body = tok[1:]
        neg = True
    m = _NUM.fullmatch(body)
    if not m or (m.group(1) == "" and m.group(2) is None):
        raise LedgerError("bad number '%s'" % tok)
    ip, fp = m.group(1), m.group(2)
    if "," in ip and not _GROUPED.fullmatch(ip):
        raise LedgerError("bad grouping '%s'" % tok)
    v = Decimal((ip.replace(",", "") or "0") + ("." + fp if fp else ""))
    return -v if neg else v


def _quant(v, dec):
    q = v.quantize(Decimal(1).scaleb(-dec), context=_CTX)
    if q == 0:
        q = Decimal(0).quantize(Decimal(1).scaleb(-dec), context=_CTX)
    return q


def _digits(q, dec):
    s = format(abs(q), "f")
    ip, _, fp = s.partition(".")
    out = "{:,}".format(int(ip))
    if dec:
        out += "." + fp
    return ("-" if q < 0 else "") + out


def _fmt(v, code, dec):
    return _digits(_quant(v, dec), dec) + " " + code


def _fmt_unit(v, code, dec):
    exp = v.as_tuple().exponent
    d = max(dec, -exp if exp < 0 else 0)
    return _digits(_quant(v, d), d) + " " + code


def _amount(toks, cur, default=None, unit=False):
    """toks: an amount part's tokens -> (value, code, NUMBER token). The value is rounded unless unit."""
    if len(toks) == 1 and default is not None:
        num, ctok = toks[0], None
    elif len(toks) != 2:
        raise LedgerError("bad amount '%s'" % " ".join(toks))
    elif toks[0][:1] in "0123456789-(.":
        num, ctok = toks
    else:
        ctok, num = toks
    v = _number(num)
    if ctok is None:
        code = default
    else:
        code = cur.names.get(ctok.lower())
        if code is None:
            raise LedgerError("unknown currency '%s'" % ctok)
    v = _quant(v, cur.decimals[code])
    if abs(v) >= _LIMIT:
        raise LedgerError("amount too large '%s'" % num)
    return v, code, num


def normalize_amount(text):
    cur = _Currencies()
    q, code, _ = _amount(_tokens(text.strip(" \t")), cur)
    return _fmt(q, code, cur.decimals[code])


# -------------------------------------------------------------------------------------- accounts

_ROOTS = {"assets": "assets", "liabilities": "liabilities", "equity": "equity", "income": "income",
          "expenses": "expenses"}
_SEG = re.compile(r"[a-z][a-z0-9-]{0,19}")


def _account(tok):
    segs = tok.split(":")
    root = _ROOTS.get(segs[0].lower())
    if root is None:
        raise LedgerError("bad account root '%s'" % segs[0])
    rest = [s.lower() for s in segs[1:]]
    for s in rest:
        if not _SEG.fullmatch(s):
            raise LedgerError("bad account '%s'" % tok)
    if len(segs) > 6:
        raise LedgerError("account too deep '%s'" % tok)
    return ":".join([root] + rest)


def normalize_account(text):
    return _account(text.strip(" \t"))


def _under(acct, anc):
    return acct.startswith(anc)


def _ancestors(acct):
    segs = acct.split(":")
    return [":".join(segs[:i]) for i in range(len(segs), 0, -1)]


# --------------------------------------------------------------------------------------- process

_TAG = re.compile(r"#[A-Za-z0-9-]{1,20}")


def _split_comment(line):
    if ";" in line:
        i = line.index(";")
        return line[:i], line[i + 1:].strip()
    return line, None


def _with_comment(s, comment):
    if comment is None:
        return s
    return s + ("  ; " + comment if comment else "  ;")


def _comment_text(line):
    s = line.lstrip(" \t")[1:].lstrip(" \t")
    return "; " + s if s else ";"


def _logical_lines(text):
    lines = []
    for p in text.split("\n"):
        if p.endswith("\r"):
            p = p[:-1]
        lines.append(p.rstrip(" \t"))
    out = []
    i = 0
    while i < len(lines):
        start = i
        cur = lines[i]
        out.append((start + 1, cur.rstrip(" \t")))
        i += 1
    return out


class _State(object):
    def __init__(self):
        self.cur = _Currencies()
        self.default = None
        self.year = None
        self.aliases = {}
        self.events = {}  # account -> "open" | "close" (latest error-free directive naming it)
        self.last_date = None
        self.totals = {}  # (account, code) -> amount sum
        self.entries = []
        self.errors = []

    def err(self, lineno, msg):
        self.errors.append((lineno, msg))

    def date(self, tok):
        return _date(tok, self.year)

    def order(self, date):
        if self.last_date is not None and date < self.last_date:
            raise LedgerError("date %s is before %s" % (date, self.last_date))

    def account(self, tok):
        segs = tok.split(":")
        target = self.aliases.get(segs[0].lower())
        if target is not None:
            tok = target + tok[len(segs[0]):]
        return _account(tok)

    def check_state(self, acct):
        for a in _ancestors(acct):
            ev = self.events.get(a)
            if ev == "open":
                return
            if ev == "close":
                raise LedgerError("account '%s' is closed" % acct)
        raise LedgerError("account '%s' is not open" % acct)

    def amount(self, toks, unit=False):
        return _amount(toks, self.cur, self.default, unit)

    def fmt(self, q, code):
        return _fmt(q, code, self.cur.decimals[code])

    def sums_under(self, acct):
        sums = {}
        for (a, c), v in self.totals.items():
            if _under(a, acct):
                sums[c] = sums.get(c, Decimal(0)) + v
        return sums

    def listing(self, sums):
        return ", ".join(self.fmt(v, c) for c, v in sorted(sums.items()) if v != 0)

    # ---- directives
    def directive(self, lineno, line):
        before, comment = _split_comment(line)
        toks = _tokens(before)
        kw = toks[0]
        handler = {"commodity": self.commodity, "default": self.default_, "year": self.year_,
                   "alias": self.alias, "open": self.open, "close": self.close,
                   "balance": self.balance}.get(kw)
        try:
            if handler is None:
                raise LedgerError("unknown directive '%s'" % kw)
            out = handler(toks)
        except LedgerError as e:
            self.err(lineno, str(e))
            return
        self.entries.append([_with_comment(out, comment)])

    def commodity(self, toks):
        if len(toks) < 3:
            raise LedgerError("bad commodity directive")
        code, dec, aliases = toks[1], toks[2], toks[3:]
        if not re.fullmatch(r"[A-Z]{2,6}", code):
            raise LedgerError("bad currency code '%s'" % code)
        if not re.fullmatch(r"[0-8]", dec):
            raise LedgerError("bad decimals '%s'" % dec)
        for a in aliases:
            if not re.fullmatch(r"[A-Za-z0-9$]{1,10}", a):
                raise LedgerError("bad alias '%s'" % a)
        seen = set(self.cur.names)
        for t in [code] + aliases:
            if t.lower() in seen:
                raise LedgerError("currency '%s' already defined" % t)
            seen.add(t.lower())
        self.cur.add(code, int(dec), aliases)
        return " ".join(["commodity", code, dec] + [a.upper() for a in aliases])

    def default_(self, toks):
        if len(toks) != 2:
            raise LedgerError("bad default directive")
        code = self.cur.names.get(toks[1].lower())
        if code is None:
            raise LedgerError("unknown currency '%s'" % toks[1])
        self.default = code
        return "default " + code

    def year_(self, toks):
        if len(toks) != 2 or not re.fullmatch(r"[0-9]{4}", toks[1]) or not 1900 <= int(toks[1]) <= 2199:
            raise LedgerError("bad year directive")
        self.year = int(toks[1])
        return "year " + toks[1]

    def alias(self, toks):
        if len(toks) != 3:
            raise LedgerError("bad alias directive")
        name = toks[1].lower()
        if not _SEG.fullmatch(name) or name in _ROOTS:
            raise LedgerError("bad alias name '%s'" % toks[1])
        acct = _account(toks[2])
        if name in self.aliases:
            raise LedgerError("alias '%s' already defined" % toks[1])
        self.aliases[name] = acct
        return "alias %s %s" % (name, acct)

    def open(self, toks):
        if len(toks) != 3:
            raise LedgerError("bad open directive")
        date = self.date(toks[1])
        self.order(date)
        acct = self.account(toks[2])
        if self.events.get(acct) == "open":
            raise LedgerError("account '%s' is already open" % acct)
        self.events[acct] = "open"
        self.last_date = date
        return "open %s %s" % (date, acct)

    def close(self, toks):
        if len(toks) != 3:
            raise LedgerError("bad close directive")
        date = self.date(toks[1])
        self.order(date)
        acct = self.account(toks[2])
        ev = self.events.get(acct)
        if ev == "close":
            raise LedgerError("account '%s' is already closed" % acct)
        if ev is None:
            raise LedgerError("account '%s' was never opened" % acct)
        left = self.listing(self.sums_under(acct))
        if left:
            raise LedgerError("cannot close %s: balance %s" % (acct, left))
        self.events[acct] = "close"
        self.last_date = date
        return "close %s %s" % (date, acct)

    def balance(self, toks):
        if len(toks) < 4:
            raise LedgerError("bad balance directive")
        date = self.date(toks[1])
        self.order(date)
        acct = self.account(toks[2])
        self.check_state(acct)
        q, code, _ = self.amount(toks[3:])
        found = self.sums_under(acct).get(code, Decimal(0))
        if found != q:
            raise LedgerError("balance mismatch for %s: expected %s, found %s" % (
                acct, self.fmt(q, code), self.fmt(found, code)))
        self.last_date = date
        return "balance %s %s %s" % (date, acct, self.fmt(q, code))

    # ---- transactions
    def posting(self, toks):
        """-> [account, amount|None, code|None, weight|None, weight code|None, price text|None]"""
        acct = self.account(toks[0])
        self.check_state(acct)
        rest = toks[1:]
        if not rest:
            return [acct, None, None, None, None, None]
        at = [i for i, x in enumerate(rest) if x in ("@", "@@")]
        if not at:
            q, code, _ = self.amount(rest)
            return [acct, q, code, q, code, None]
        i = at[0]
        if len(at) > 1:
            raise LedgerError("bad price")
        if i == 0:
            raise LedgerError("price without amount")
        q, code, _ = self.amount(rest[:i])
        unit = rest[i] == "@"
        p, pcode, ptok = self.amount(rest[i + 1:], unit=unit)
        if ptok[:1] in ("-", "("):
            raise LedgerError("negative price '%s'" % ptok)
        if pcode == code:
            raise LedgerError("price in same currency")
        pdec = self.cur.decimals[pcode]
        if unit:
            w = _quant(_CTX.multiply(q, p), pdec)
            ptext = "@ " + _fmt_unit(p, pcode, pdec)
        else:
            w = -p if q < 0 else p
            ptext = "@@ " + self.fmt(p, pcode)
        return [acct, q, code, w, pcode, ptext]

    def transaction(self, header, body):
        lineno, line = header
        before, comment = _split_comment(line)
        toks = _tokens(before)
        try:
            date = self.date(toks[0])
            self.order(date)
            rest = toks[1:]
            flag = "*"
            if rest and rest[0] in ("*", "!", "txn"):
                flag = "!" if rest[0] == "!" else "*"
                rest = rest[1:]
            tags = []
            desc = " ".join(rest)
            if not desc:
                raise LedgerError("missing description")
        except LedgerError as e:
            self.err(lineno, str(e))
            return
        items = []  # ("c", text) | ["p", posting, comment]
        bad = False
        for ln, bl in body:
            if bl.lstrip(" \t")[:1] in (";", "#"):
                items.append(("c", "  " + _comment_text(bl)))
                continue
            b, c = _split_comment(bl)
            try:
                p = self.posting(_tokens(b))
            except LedgerError as e:
                self.err(ln, str(e))
                bad = True
                continue
            items.append(["p", p, c])
        if bad:
            return
        posts = [it[1] for it in items if it[0] == "p"]
        try:
            if len(posts) < 2:
                raise LedgerError("transaction needs at least two postings")
            elided = [p for p in posts if p[1] is None]
            if len(elided) > 1:
                raise LedgerError("more than one posting without amount")
            if elided:
                codes = set(p[4] for p in posts if p[1] is not None)
                if len(codes) != 1:
                    raise LedgerError("cannot infer amount with several currencies")
                code = codes.pop()
                s = sum((p[3] for p in posts if p[1] is not None), Decimal(0))
                v = _quant(-s, self.cur.decimals[code])
                elided[0][1:5] = [v, code, v, code]
            sums = {}
            for p in posts:
                sums[p[4]] = sums.get(p[4], Decimal(0)) + p[3]
            off = self.listing(sums)
            if off:
                raise LedgerError("transaction does not balance: " + off)
        except LedgerError as e:
            self.err(lineno, str(e))
            return
        self.last_date = date
        for p in posts:
            k = (p[0], p[2])
            self.totals[k] = self.totals.get(k, Decimal(0)) + p[1]
        head = "%s %s %s" % (date, flag, desc) + "".join(" #" + t for t in tags)
        out = [_with_comment(head, comment)]
        for it in items:
            if it[0] == "c":
                out.append(it[1])
                continue
            p = it[1]
            num, code = self.fmt(p[1], p[2]).split(" ")
            lead = "  " + p[0]
            pad = max(2, 50 - len(lead) - len(num))
            text = lead + " " * pad + num + " " + code
            if p[5]:
                text += " " + p[5]
            out.append(_with_comment(text, it[2]))
        self.entries.append(out)


def process(text):
    st = _State()
    txn = None  # [header, body]
    block = None  # list of comment lines

    def close_txn():
        if txn is not None:
            st.transaction(txn[0], txn[1])

    def close_block():
        if block:
            st.entries.append(block)

    for lineno, line in _logical_lines(text):
        if line.strip(" \t") == "":
            close_txn()
            txn = None
            close_block()
            block = None
            continue
        indented = line[0] in " \t"
        comment = line.lstrip(" \t")[0] in ";#"
        if indented and txn is not None:
            txn[1].append((lineno, line))
            continue
        if not indented:
            close_txn()
            txn = None
        if comment:
            if block is None:
                block = []
            block.append(_comment_text(line))
            continue
        close_block()
        block = None
        if indented:
            st.err(lineno, "unexpected indented line")
        elif line[0] in "0123456789":
            txn = [(lineno, line), []]
        else:
            st.directive(lineno, line)
    close_txn()
    close_block()
    errors = ["line %d: %s" % e for e in sorted(st.errors, key=lambda e: e[0])]
    if False:
        errors = errors[:_MAX_ERRORS] + ["too many errors: %d more" % (len(errors) - _MAX_ERRORS)]
    output = "\n\n".join("\n".join(e) for e in st.entries)
    return Result(output + "\n" if output else "", errors)
