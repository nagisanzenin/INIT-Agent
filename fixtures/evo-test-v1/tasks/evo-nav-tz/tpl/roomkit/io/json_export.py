"""The json exporter: an array of booking objects in the order given (indent 2, final newline)."""
import json

from roomkit.util.timefmt import fmt_dt

OPTIONS = ()
REQUIRED = ()


def booking_object(b):
    obj = {
        "id": b.id,
        "room": b.room,
        "start": fmt_dt(b.start),
        "end": fmt_dt(b.end),
        "title": b.title,
        "owner": b.owner,
        "attendees": b.attendees,
    }
#@A
    obj["tz"] = b.tz
#@E
    return obj


def export_json(bookings, options=None):
    return json.dumps([booking_object(b) for b in bookings], indent=2) + "\n"
