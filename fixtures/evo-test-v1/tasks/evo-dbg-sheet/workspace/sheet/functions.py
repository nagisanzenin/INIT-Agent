"""Built-in functions (SPEC.md section 7). Each takes a list of evaluator Args and returns a value."""
from decimal import ROUND_HALF_UP, Decimal, localcontext

from .values import (BLANK, DIV0, VALUE, is_bool, is_error, is_number, is_text, parse_number, to_bool, to_number,
                     to_text)

MANY = 255


def items(args):
    """(value, referenced) for every value the arguments supply, in order: a reference argument supplies the
    values of its cells (referenced=True), any other argument its own value (referenced=False)."""
    for a in args:
        if a.is_ref:
            for v in a.cells():
                yield v, True
        else:
            yield a.value(), False


def numbers(args):
    """The numbers SUM, AVERAGE, MIN and MAX work on, or the first error met."""
    out = []
    for v, referenced in items(args):
        if is_error(v):
            return v
        if referenced:
            if is_number(v):
                out.append(v)
        else:
            x = to_number(v)
            if is_error(x):
                return x
            out.append(x)
    return out


def logicals(args):
    """The logical values AND and OR work on, or an error."""
    out = []
    for v, referenced in items(args):
        if is_error(v):
            return v
        if referenced:
            if is_bool(v):
                out.append(v)
            elif is_number(v):
                out.append(v != 0)
        else:
            b = to_bool(v)
            if is_error(b):
                return b
            out.append(b)
    if not out:
        return VALUE
    return out


def fn_sum(args):
    xs = numbers(args)
    if is_error(xs):
        return xs
    return float(sum(xs))


def fn_average(args):
    xs = numbers(args)
    if is_error(xs):
        return xs
    if not xs:
        return DIV0
    return sum(xs) / len(xs)


def fn_min(args):
    xs = numbers(args)
    if is_error(xs):
        return xs
    return min(xs) if xs else 0.0


def fn_max(args):
    xs = numbers(args)
    if is_error(xs):
        return xs
    return max(xs) if xs else 0.0


def fn_count(args):
    return sum(1 for v, _ in items(args) if is_number(v))


def fn_counta(args):
    return sum(1 for v, _ in items(args) if v is not BLANK)


def fn_if(args):
    cond = to_bool(args[0].value())
    if is_error(cond):
        return cond
    if cond:
        return args[1].value()
    if len(args) == 3:
        return args[2].value()
    return False


def fn_and(args):
    bs = logicals(args)
    if is_error(bs):
        return bs
    return all(bs)


def fn_or(args):
    bs = logicals(args)
    if is_error(bs):
        return bs
    return any(bs)


def fn_not(args):
    b = to_bool(args[0].value())
    if is_error(b):
        return b
    return not b


def round_half_away(x, digits):
    """x rounded to `digits` decimal places, halves away from zero, judged on the shortest decimal form of x."""
    with localcontext() as ctx:
        ctx.prec = 800
        q = Decimal(1).scaleb(-digits)
        return float(Decimal(repr(float(x))).quantize(q, rounding=ROUND_HALF_UP))


def fn_round(args):
    x = to_number(args[0].value())
    if is_error(x):
        return x
    digits = 0
    if len(args) == 2:
        d = to_number(args[1].value())
        if is_error(d):
            return d
        digits = max(-400, min(400, int(d)))
    return round_half_away(x, digits)


def fn_abs(args):
    x = to_number(args[0].value())
    if is_error(x):
        return x
    return abs(x)


def text_arg(arg):
    v = arg.value()
    if is_error(v):
        return v
    return to_text(v)


def fn_len(args):
    s = text_arg(args[0])
    if is_error(s):
        return s
    return len(s)


def fn_upper(args):
    s = text_arg(args[0])
    if is_error(s):
        return s
    return s.upper()


def fn_concat(args):
    out = []
    for v, _ in items(args):
        if is_error(v):
            return v
        out.append(to_text(v))
    return "".join(out)


CRITERIA_OPS = ("<=", ">=", "<>", "<", ">", "=")
ORDER = {"<": lambda c: c < 0, "<=": lambda c: c <= 0, ">": lambda c: c > 0, ">=": lambda c: c >= 0}


def _cmp(a, b):
    return (a > b) - (a < b)


def _is_empty(v):
    return v is BLANK or v == ""


def criterion(c):
    """A predicate on cell values for a SUMIF/COUNTIF criterion value (SPEC.md section 7.3)."""
    if is_bool(c):
        return lambda v: is_bool(v) and v == c
    if is_number(c):
        return lambda v: is_number(v) and v == c
    if c is BLANK:
        return _is_empty
    op, s = "=", c
    for o in CRITERIA_OPS:
        if c.startswith(o):
            op, s = o, c[len(o):]
            break
    if s == "":
        if op == "=":
            return _is_empty
        if op == "<>":
            return lambda v: not _is_empty(v)
        return lambda v: False
    x = parse_number(s)
    if x is not None:
        def same(v):
            return is_number(v) and v == x

        def ordered(v):
            return is_number(v) and ORDER[op](_cmp(v, x))
    elif s.upper() in ("TRUE", "FALSE"):
        b = s.upper() == "TRUE"

        def same(v):
            return is_bool(v) and v == b

        def ordered(v):
            return is_bool(v) and ORDER[op](_cmp(v, b))
    else:
        t = s.lower()

        def same(v):
            return is_text(v) and v.lower() == t

        def ordered(v):
            return is_text(v) and ORDER[op](_cmp(v.lower(), t))
    if op == "=":
        return same
    if op == "<>":
        return lambda v: not same(v)
    return ordered


def matches(args):
    """For SUMIF and COUNTIF: one flag per cell of the range argument telling whether the cell meets the
    criterion (error cells never do), or an error."""
    if not args[0].is_ref:
        return VALUE
    c = args[1].value()
    if is_error(c):
        return c
    pred = criterion(c)
    return [not is_error(v) and pred(v) for v in args[0].cells()]


def fn_countif(args):
    flags = matches(args)
    if is_error(flags):
        return flags
    return sum(1 for f in flags if f)


def fn_sumif(args):
    flags = matches(args)
    if is_error(flags):
        return flags
    target = args[-1] if len(args) == 3 else args[0]
    if not target.is_ref or target.rect.shape != args[0].rect.shape:
        return VALUE
    total = 0.0
    for f, v in zip(flags, target.cells()):
        if f:
            if is_error(v):
                return v
            if is_number(v):
                total += v
    return total


FUNCTIONS = {
    "SUM": (1, MANY, fn_sum),
    "AVERAGE": (1, MANY, fn_average),
    "MIN": (1, MANY, fn_min),
    "MAX": (1, MANY, fn_max),
    "COUNT": (1, MANY, fn_count),
    "COUNTA": (1, MANY, fn_counta),
    "IF": (2, 3, fn_if),
    "AND": (1, MANY, fn_and),
    "OR": (1, MANY, fn_or),
    "NOT": (1, 1, fn_not),
    "ROUND": (1, 2, fn_round),
    "ABS": (1, 1, fn_abs),
    "LEN": (1, 1, fn_len),
    "UPPER": (1, 1, fn_upper),
    "CONCAT": (1, MANY, fn_concat),
    "SUMIF": (2, 3, fn_sumif),
    "COUNTIF": (2, 2, fn_countif),
}
