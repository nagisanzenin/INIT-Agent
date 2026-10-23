"""Parsing and formatting of datetimes, dates and clock times, and small time arithmetic.

Formats (README.md, "Times"): datetime `YYYY-MM-DD HH:MM` (a `T` may replace the space on input), date `YYYY-MM-DD`,
clock time `HH:MM`. All datetimes are naive.
"""
import re
from datetime import date, datetime, time, timedelta

from roomkit.errors import ParseError

DT_FORMAT = "%Y-%m-%d %H:%M"
_DT_RE = re.compile(r"^(\d{4})-(\d\d)-(\d\d)[ T](\d\d):(\d\d)$")
_DATE_RE = re.compile(r"^(\d{4})-(\d\d)-(\d\d)$")
_HHMM_RE = re.compile(r"^(\d\d):(\d\d)$")


def parse_dt(text):
    """'2026-03-02 09:00' -> datetime(2026, 3, 2, 9, 0)."""
    s = text.strip() if isinstance(text, str) else ""
    m = _DT_RE.match(s)
    if m:
        try:
            return datetime(*[int(g) for g in m.groups()])
        except ValueError:
            pass
    raise ParseError("bad datetime '%s' (want YYYY-MM-DD HH:MM)" % (text,))


def fmt_dt(dt):
    return "%04d-%02d-%02d %02d:%02d" % (dt.year, dt.month, dt.day, dt.hour, dt.minute)


def parse_date(text):
    s = text.strip() if isinstance(text, str) else ""
    m = _DATE_RE.match(s)
    if m:
        try:
            return date(*[int(g) for g in m.groups()])
        except ValueError:
            pass
    raise ParseError("bad date '%s' (want YYYY-MM-DD)" % (text,))


def fmt_date(d):
    return "%04d-%02d-%02d" % (d.year, d.month, d.day)


def parse_hhmm(text):
    s = text.strip() if isinstance(text, str) else ""
    m = _HHMM_RE.match(s)
    if m:
        h, mi = int(m.group(1)), int(m.group(2))
        if h < 24 and mi < 60:
            return time(h, mi)
    raise ParseError("bad time '%s' (want HH:MM)" % (text,))


def fmt_hhmm(t):
    return "%02d:%02d" % (t.hour, t.minute)


def minutes_between(a, b):
    """Whole minutes from a to b (negative when b is before a)."""
    return int((b - a).total_seconds() // 60)


def add_minutes(dt, n):
    return dt + timedelta(minutes=n)


def combine(d, t):
    return datetime(d.year, d.month, d.day, t.hour, t.minute)


def day_start(d):
    return datetime(d.year, d.month, d.day)


def minute_of_day(t):
    """Minutes since midnight of a time or datetime."""
    return t.hour * 60 + t.minute


def date_range(first, last):
    """Every date from first to last, both included (empty when last < first)."""
    out = []
    d = first
    while d <= last:
        out.append(d)
        d += timedelta(days=1)
    return out


def overlap_minutes(a_start, a_end, b_start, b_end):
    """Minutes shared by [a_start, a_end) and [b_start, b_end); 0 when they do not overlap."""
    lo = max(a_start, b_start)
    hi = min(a_end, b_end)
    if hi <= lo:
        return 0
    return minutes_between(lo, hi)


def span_label(start, end):
    """'09:00-10:00'; an end on a later day than the start gets '+1' ('23:30-00:30+1')."""
    label = "%s-%s" % (fmt_hhmm(start), fmt_hhmm(end))
    if end.date() > start.date():
        label += "+1"
    return label


def end_label(start, end):
    """'10:00', or '00:30+1' when the end is on a later day than the start."""
    label = fmt_hhmm(end)
    if end.date() > start.date():
        label += "+1"
    return label


def floor_hour(dt):
    return dt.replace(minute=0, second=0, microsecond=0)
