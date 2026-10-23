"""Field rules: one table per record kind, checked in table order.

Each field has a tuple of rules; a rule is a name with an optional argument after ':'. Values arrive as text
(importers, store records, command-line arguments) or as JSON values (the request handler). check_fields returns
the converted values of the fields that are present; a field that is absent or empty is skipped unless it is
"required". The first broken rule raises FieldError(field, message).

    required    the field must be present and not empty           missing field 'F'
    slug        lowercase letters, digits, '-' (1-30)            bad value 'X' for F (want a-z, 0-9, -)
    datetime    YYYY-MM-DD HH:MM                                  bad datetime 'X' (want YYYY-MM-DD HH:MM)
    hhmm        HH:MM                                             bad time 'X' (want HH:MM)
    int         an integer                                        bad integer 'X' for F
    min:N       an integer >= N                                   F must be >= N
    maxlen:N    at most N characters                              F longer than N characters
    tags        'a;b' -> ['a', 'b']
#@A
    zone        a zone code or literal offset -> its label        unknown zone 'X'
#@E
"""
import re

from roomkit.errors import FieldError, ParseError
from roomkit.util.text import is_slug, split_tags
from roomkit.util.timefmt import parse_dt, parse_hhmm
#@A
from roomkit.util.zones import parse_zone
#@E

BOOKING_RULES = (
    ("room", ("required", "slug")),
    ("start", ("required", "datetime")),
    ("end", ("required", "datetime")),
    ("title", ("required", "maxlen:80")),
    ("owner", ("required", "slug")),
    ("attendees", ("int", "min:0")),
#@A
    ("tz", ("zone",)),
#@E
)

ROOM_RULES = (
    ("id", ("required", "slug")),
    ("name", ("required", "maxlen:40")),
    ("capacity", ("required", "int", "min:1")),
    ("opens", ("hhmm",)),
    ("closes", ("hhmm",)),
    ("tags", ("tags",)),
#@A
    ("tz", ("zone",)),
#@E
)

_INT_RE = re.compile(r"^[+-]?\d+$")


def _text(field, value):
    """A raw value as text: strings stripped, ints as digits, lists (tags) joined with ';'."""
    if value is None:
        return ""
    if isinstance(value, bool):
        raise FieldError(field, "bad value '%s' for %s" % (value, field))
    if isinstance(value, int):
        return str(value)
    if isinstance(value, list) and all(isinstance(x, str) for x in value):
        return ";".join(value)
    if isinstance(value, str):
        return value.strip()
    raise FieldError(field, "bad value '%s' for %s" % (value, field))


def _parse(field, fn, text):
    try:
        return fn(text)
    except ParseError as e:
        raise FieldError(field, e.message)


def apply_rule(field, rule, value):
    """Apply one rule to the current value (text or an already converted value); return the new value."""
    name, _, arg = rule.partition(":")
    if name == "required":
        return value
    if name == "slug":
        if not is_slug(value):
            raise FieldError(field, "bad value '%s' for %s (want a-z, 0-9, -)" % (value, field))
        return value
    if name == "datetime":
        return _parse(field, parse_dt, value)
    if name == "hhmm":
        return _parse(field, parse_hhmm, value)
    if name == "int":
        if not _INT_RE.match(value):
            raise FieldError(field, "bad integer '%s' for %s" % (value, field))
        return int(value)
    if name == "min":
        if value < int(arg):
            raise FieldError(field, "%s must be >= %s" % (field, arg))
        return value
    if name == "maxlen":
        if len(value) > int(arg):
            raise FieldError(field, "%s longer than %s characters" % (field, arg))
        return value
    if name == "tags":
        return split_tags(value)
#@A
    if name == "zone":
        return _parse(field, parse_zone, value)
#@E
    raise ValueError("unknown field rule %r" % rule)


def check_fields(rules, raw):
    """Check a dict of raw values against a rule table; return {field: converted value} for present fields."""
    out = {}
    for field, field_rules in rules:
        text = _text(field, raw.get(field))
        if text == "":
            if "required" in field_rules:
                raise FieldError(field, "missing field '%s'" % field)
            continue
        value = text
        for rule in field_rules:
            value = apply_rule(field, rule, value)
        out[field] = value
    return out


def field_names(rules):
    return tuple(f for f, _ in rules)
