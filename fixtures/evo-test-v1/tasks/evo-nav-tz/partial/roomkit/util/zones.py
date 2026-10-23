"""Fixed-offset time zones (CHANGE.md, "Zones"). A zone label is a code from ZONES ("JST") or a literal offset
("+05:30"). Datetimes stay naive; a label says which offset they are written in. There is no DST."""
import re
from datetime import timedelta

from roomkit.errors import ZoneError

ZONES = {
    "UTC": 0, "CET": 60, "EET": 120, "MSK": 180,
    "IST": 330, "NPT": 345, "ICT": 420, "JST": 540,
    "ACST": 570, "AEST": 600, "NZST": 720, "BRT": -180,
    "EST": -300, "MST": -420, "PST": -480, "HST": -600,
}
DEFAULT_ZONE = "UTC"
MIN_OFFSET = -12 * 60
MAX_OFFSET = 14 * 60
_LITERAL = re.compile(r"^([+-])(\d\d):(\d\d)$")
_INLINE = re.compile(r"^(.*\d\d:\d\d)([+-]\d\d:\d\d)$")


def _literal_minutes(text):
    m = _LITERAL.match(text)
    if not m:
        return None
    hh, mm = int(m.group(2)), int(m.group(3))
    if mm not in (0, 15, 30, 45):
        return None
    total = hh * 60 + mm
    if m.group(1) == "-":
        total = -total
    if total < MIN_OFFSET or total > MAX_OFFSET:
        return None
    return total


def parse_zone(text):
    """'jst' -> 'JST', '+05:30' -> '+05:30'; anything else raises ZoneError."""
    if isinstance(text, str):
        if text.upper() in ZONES:
            return text.upper()
        if _literal_minutes(text) is not None:
            return text
    raise ZoneError("unknown zone '%s'" % (text,))


def offset_minutes(label):
    if label in ZONES:
        return ZONES[label]
    m = _literal_minutes(label) if isinstance(label, str) else None
    if m is None:
        raise ZoneError("unknown zone '%s'" % (label,))
    return m


def fmt_offset(minutes):
    sign = "-" if minutes < 0 else "+"
    a = abs(minutes)
    return "%s%02d:%02d" % (sign, a // 60, a % 60)


def to_utc(dt, label):
    return dt - timedelta(minutes=offset_minutes(label))


def from_utc(dt, label):
    return dt + timedelta(minutes=offset_minutes(label))


def convert(dt, src, dst):
    if src == dst:
        return dt
    return from_utc(to_utc(dt, src), dst)


def split_inline(text):
    """'2026-03-02 09:00+05:30' -> ('2026-03-02 09:00', '+05:30'); without an inline offset -> (text, None)."""
    m = _INLINE.match(text.strip())
    if not m:
        return text, None
    return m.group(1), m.group(2)


def describe(label):
    """'JST (+09:00)'."""
    return "%s (%s)" % (label, fmt_offset(offset_minutes(label)))
