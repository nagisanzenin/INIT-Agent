"""The csv exporter: one row per booking, in the order given (the CLI passes them sorted by id)."""
from roomkit.util.csvio import to_text
from roomkit.util.timefmt import fmt_dt

OPTIONS = ()
REQUIRED = ()
#@B
COLUMNS = ("id", "room", "start", "end", "title", "owner", "attendees")
#@A
COLUMNS = ("id", "room", "start", "end", "title", "owner", "attendees", "tz")
#@E


def row_of(b):
    rec = {
        "id": str(b.id),
        "room": b.room,
        "start": fmt_dt(b.start),
        "end": fmt_dt(b.end),
        "title": b.title,
        "owner": b.owner,
        "attendees": str(b.attendees),
    }
#@A
    rec["tz"] = b.tz
#@E
    return [rec[c] for c in COLUMNS]


def export_csv(bookings, options=None):
    return to_text(COLUMNS, [row_of(b) for b in bookings])
