"""Turning command-line values into typed values. Any bad value is a UsageError (exit 2)."""
from roomkit.errors import FieldError, ParseError, UsageError
from roomkit.util.text import parse_key_value
from roomkit.util.timefmt import parse_date, parse_dt
#@A
from roomkit.util.zones import parse_zone
#@E


def usage(fn, *a, **kw):
    """Call fn; a ParseError (FieldError included) becomes a UsageError with the same message."""
    try:
        return fn(*a, **kw)
    except (ParseError, FieldError) as e:
        raise UsageError(e.message)


def arg_dt(text):
    return usage(parse_dt, text)


def arg_date(text):
    return usage(parse_date, text)


def arg_int(text, name, minimum=None):
    try:
        n = int(text)
    except (TypeError, ValueError):
        raise UsageError("bad integer '%s' for %s" % (text, name))
    if minimum is not None and n < minimum:
        raise UsageError("%s must be >= %d" % (name, minimum))
    return n


def arg_booking_id(text):
    return arg_int(text, "id", 1)


#@A
def arg_zone(text):
    """None stays None (no --tz given)."""
    if text is None:
        return None
    return usage(parse_zone, text)


#@E
def arg_options(pairs):
    """['tz=JST', 'room=atlas'] -> {'tz': 'JST', 'room': 'atlas'}."""
    out = {}
    for item in pairs:
        kv = parse_key_value(item)
        if kv is None:
            raise UsageError("bad option '%s' (want KEY=VALUE)" % item)
        out[kv[0]] = kv[1]
    return out


def given(**values):
    """The keyword arguments that are not None (options given on the command line)."""
    return {k: v for k, v in values.items() if v is not None}
