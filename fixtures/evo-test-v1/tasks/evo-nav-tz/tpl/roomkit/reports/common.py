"""Helpers shared by the reports: reading options (all options arrive as text) and small row tools."""
from decimal import ROUND_HALF_UP, Decimal

from roomkit.util.timefmt import parse_date
#@A
from roomkit.util.zones import DEFAULT_ZONE, parse_zone
#@E


def opt_date(options, key):
    """The option as a date, None when it is absent or empty."""
    v = options.get(key)
    if v in (None, ""):
        return None
    return parse_date(v)


def opt_text(options, key):
    v = options.get(key)
    return v if v not in (None, "") else None


#@A
def opt_zone(options):
    """The display zone of a report: the option `tz`, UTC when absent."""
    v = options.get("tz")
    return DEFAULT_ZONE if v in (None, "") else parse_zone(v)


#@E
def in_range(d, first, last):
    return (first is None or d >= first) and (last is None or d <= last)


def percent(part, whole):
    """part x 100 / whole rounded half up to one decimal, as text ("37.5"); "0.0" when whole is 0."""
    if whole <= 0:
        return "0.0"
    q = (Decimal(part) * 100 / Decimal(whole)).quantize(Decimal("0.1"), rounding=ROUND_HALF_UP)
    return str(q)


def columns(rows):
    """The keys of the first row, in order (the table header for a list of rows)."""
    return list(rows[0].keys()) if rows else []
