"""Records: flat dicts of strings, the form CSV store rows take (and the field rules check).

room_to_record / booking_to_record write every field as text (tags joined with ';'); the *_from_record
functions read them back and raise ParseError (a FieldError naming the field) for a bad value.
"""
from roomkit.errors import FieldError, ParseError
from roomkit.models.booking import Booking
from roomkit.models.history import HistoryEntry
from roomkit.models.room import Room
from roomkit.models.waitlist import WaitEntry
from roomkit.util.text import join_tags, split_tags
from roomkit.util.timefmt import fmt_dt, fmt_hhmm, parse_dt, parse_hhmm
#@A
from roomkit.util.zones import parse_zone
#@E

#@B
ROOM_FIELDS = ("id", "name", "capacity", "opens", "closes", "tags")
BOOKING_FIELDS = ("id", "room", "start", "end", "title", "owner", "attendees")
#@A
ROOM_FIELDS = ("id", "name", "capacity", "opens", "closes", "tags", "tz")
BOOKING_FIELDS = ("id", "room", "start", "end", "title", "owner", "attendees", "tz")
#@E


def _int(rec, key):
    raw = rec.get(key, "")
    try:
        return int(raw)
    except (TypeError, ValueError):
        raise FieldError(key, "bad integer '%s' for %s" % (raw, key))


def _field(rec, key, fn):
    try:
        return fn(rec.get(key, ""))
    except ParseError as e:
        raise FieldError(key, e.message)


def room_to_record(room):
    return {
        "id": room.id,
        "name": room.name,
        "capacity": str(room.capacity),
        "opens": fmt_hhmm(room.opens),
        "closes": fmt_hhmm(room.closes),
        "tags": join_tags(room.tags),
#@A
        "tz": room.tz,
#@E
    }


def room_from_record(rec):
    return Room(
        id=rec.get("id", ""),
        name=rec.get("name", ""),
        capacity=_int(rec, "capacity"),
        opens=_field(rec, "opens", parse_hhmm),
        closes=_field(rec, "closes", parse_hhmm),
        tags=split_tags(rec.get("tags", "")),
#@A
        tz=_field(rec, "tz", parse_zone),
#@E
    )


def booking_to_record(b):
    return {
        "id": str(b.id),
        "room": b.room,
        "start": fmt_dt(b.start),
        "end": fmt_dt(b.end),
        "title": b.title,
        "owner": b.owner,
        "attendees": str(b.attendees),
#@A
        "tz": b.tz,
#@E
    }


def booking_from_record(rec):
    return Booking(
        id=_int(rec, "id"),
        room=rec.get("room", ""),
        start=_field(rec, "start", parse_dt),
        end=_field(rec, "end", parse_dt),
        title=rec.get("title", ""),
        owner=rec.get("owner", ""),
        attendees=_int(rec, "attendees"),
#@A
        tz=_field(rec, "tz", parse_zone),
#@E
    )


def wait_to_record(e):
    """A waitlist entry is written like a booking."""
    return booking_to_record(e)


def wait_from_record(rec):
    b = booking_from_record(rec)
#@B
    return WaitEntry(b.id, b.room, b.start, b.end, b.title, b.owner, b.attendees)
#@A
    return WaitEntry(b.id, b.room, b.start, b.end, b.title, b.owner, b.attendees, b.tz)
#@E


def history_to_record(h):
    rec = {"seq": str(h.seq), "action": h.action, "booking": str(h.booking), "room": h.room,
           "start": fmt_dt(h.start), "end": fmt_dt(h.end)}
#@A
    rec["tz"] = h.tz
#@E
    return rec


def history_from_record(rec):
    return HistoryEntry(seq=_int(rec, "seq"), action=rec.get("action", ""), booking=_int(rec, "booking"),
                        room=rec.get("room", ""), start=_field(rec, "start", parse_dt), end=_field(rec, "end", parse_dt),
#@A
                        tz=_field(rec, "tz", parse_zone),
#@E
                        )


def record_row(rec, fields):
    """The values of a record in column order (missing fields as '')."""
    return [rec.get(f, "") for f in fields]
