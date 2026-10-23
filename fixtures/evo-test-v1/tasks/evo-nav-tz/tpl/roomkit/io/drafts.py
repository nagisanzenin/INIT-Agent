"""Drafts: bookings read by an importer, before they are merged into a calendar (they have no id yet).

Every importer builds a dict of raw text values per booking and calls make_draft, which applies the field rules
and puts the importer's location on any error ("line 3: bad datetime ..."). `where` stays on the draft so that
merge problems can name the place too.
"""
from dataclasses import dataclass
from datetime import datetime
#@A
from typing import Optional
#@E

from roomkit.errors import FieldError, ParseError
from roomkit.models.booking import Booking
from roomkit.validate.fields import BOOKING_RULES, check_fields


@dataclass
class Draft:
    room: str
    start: datetime
    end: datetime
    title: str
    owner: str
    attendees: int = 1
#@A
    tz: Optional[str] = None
#@E
    where: str = ""


def make_draft(raw, where, field_where=None):
    """A Draft from raw values; errors carry `where`, or field_where(field) when given."""
    try:
        vals = check_fields(BOOKING_RULES, raw)
    except FieldError as e:
        loc = field_where(e.field) if field_where is not None else where
        raise ParseError(e.message).at(loc)
    return Draft(room=vals["room"], start=vals["start"], end=vals["end"], title=vals["title"],
                 owner=vals["owner"], attendees=vals.get("attendees", 1),
#@A
                 tz=vals.get("tz"),
#@E
                 where=where)


#@B
def to_booking(d, booking_id=None):
    return Booking(booking_id, d.room, d.start, d.end, d.title, d.owner, d.attendees)
#@A
def to_booking(d, booking_id=None, tz=None):
    """A Booking from a draft; `tz` is the zone to use when the draft has none (the caller decides)."""
    return Booking(booking_id, d.room, d.start, d.end, d.title, d.owner, d.attendees, d.tz or tz or "UTC")
#@E


def raw_from_json(obj, keys, where):
    """Raw text values from a JSON object; unknown keys and non-scalar values are parse errors."""
    raw = {}
    for k, v in obj.items():
        if k not in keys:
            raise ParseError("unknown key '%s'" % k).at(where)
        if isinstance(v, bool) or not isinstance(v, (str, int)):
            raise ParseError("bad value for '%s'" % k).at(where)
        raw[k] = str(v)
    return raw
