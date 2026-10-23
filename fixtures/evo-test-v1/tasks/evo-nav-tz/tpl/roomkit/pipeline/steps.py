"""Pipeline steps. Each make_<step>(spec) checks its config and returns a function list[Booking] -> list[Booking].
Config problems raise ConfigError; the runner adds the step's number and name ("step 2 (where): ...")."""
import operator

from roomkit.errors import ConfigError
from roomkit.util.timefmt import add_minutes, fmt_dt
#@A
from roomkit.errors import ZoneError
from roomkit.util.zones import parse_zone
#@E

#@B
WHERE_FIELDS = ("id", "room", "title", "owner", "attendees", "start", "end")
#@A
WHERE_FIELDS = ("id", "room", "title", "owner", "attendees", "start", "end", "tz")
#@E
INT_FIELDS = ("id", "attendees")
OPS = {
    "eq": operator.eq,
    "ne": operator.ne,
    "lt": operator.lt,
    "le": operator.le,
    "gt": operator.gt,
    "ge": operator.ge,
    "contains": lambda have, want: want in have,
}
SORT_KEYS = ("id", "room", "owner", "start")


def _int(spec, key):
    v = spec.get(key)
    if isinstance(v, bool):
        raise ConfigError("bad %s '%s'" % (key, v))
    try:
        return int(v)
    except (TypeError, ValueError):
        raise ConfigError("bad %s '%s'" % (key, v))


def _value(b, field):
    if field in ("start", "end"):
        return fmt_dt(getattr(b, field))
    return getattr(b, field)


def make_where(spec):
    field = spec.get("field")
    op = spec.get("op", "eq")
    if field not in WHERE_FIELDS:
        raise ConfigError("unknown field '%s'" % field)
    if op not in OPS:
        raise ConfigError("unknown op '%s'" % op)
    if "value" not in spec:
        raise ConfigError("missing value")
    if field in INT_FIELDS:
        if op == "contains":
            raise ConfigError("op 'contains' needs a text field")
        want = _int(spec, "value")
    else:
        want = str(spec["value"])
    test = OPS[op]

    def where(bookings):
        return [b for b in bookings if test(_value(b, field), want)]
    return where


def make_sort(spec):
    by = spec.get("by", "id")
    desc = bool(spec.get("desc", False))
    if by not in SORT_KEYS:
        raise ConfigError("unknown sort key '%s'" % by)

    def key(b):
        if by == "start":
#@B
            return b.start
#@A
            return b.utc_start()
#@E
        return getattr(b, by)

    def sort(bookings):
        by_id = sorted(bookings, key=lambda b: b.id)
        return sorted(by_id, key=key, reverse=desc)
    return sort


def make_limit(spec):
    n = _int(spec, "n")
    if n < 0:
        raise ConfigError("bad n '%s'" % n)

    def limit(bookings):
        return list(bookings)[:n]
    return limit


def make_shift(spec):
    minutes = _int(spec, "minutes")

    def shift(bookings):
        return [b.copy(start=add_minutes(b.start, minutes), end=add_minutes(b.end, minutes)) for b in bookings]
    return shift


def make_rename_room(spec):
    old, new = spec.get("from"), spec.get("to")
    if not isinstance(old, str) or not isinstance(new, str) or not old or not new:
        raise ConfigError("rename_room needs \"from\" and \"to\"")

    def rename(bookings):
        return [b.copy(room=new) if b.room == old else b for b in bookings]
    return rename


#@A
def make_to_zone(spec):
    try:
        zone = parse_zone(spec.get("tz"))
    except ZoneError as e:
        raise ConfigError(e.message)

    def to_zone(bookings):
        out = []
        for b in bookings:
            start, end = b.in_zone(zone)
            out.append(b.copy(start=start, end=end, tz=zone))
        return out
    return to_zone
#@E
