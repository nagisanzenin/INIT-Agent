"""What the command line prints: JSON documents, tables, plain lines."""
import json
import sys

from roomkit.util.table import render
from roomkit.util.timefmt import fmt_dt, fmt_hhmm

#@B
ROOM_HEADERS = ("id", "name", "capacity", "hours", "tags")
BOOKING_HEADERS = ("id", "room", "start", "end", "title", "owner", "attendees")
#@A
ROOM_HEADERS = ("id", "name", "capacity", "hours", "tags", "tz")
BOOKING_HEADERS = ("id", "room", "start", "end", "title", "owner", "attendees", "tz")
#@E
#@B
WAIT_HEADERS = ("id", "room", "start", "end", "title", "owner", "attendees")
#@A
WAIT_HEADERS = ("id", "room", "start", "end", "title", "owner", "attendees", "tz")
#@E
#@B
HISTORY_HEADERS = ("seq", "action", "booking", "room", "start", "end")
#@A
HISTORY_HEADERS = ("seq", "action", "booking", "room", "start", "end", "tz")
#@E


def room_dict(r):
    d = {"id": r.id, "name": r.name, "capacity": r.capacity, "opens": fmt_hhmm(r.opens),
         "closes": fmt_hhmm(r.closes), "tags": list(r.tags)}
#@A
    d["tz"] = r.tz
#@E
    return d


def booking_dict(b):
    d = {"id": b.id, "room": b.room, "start": fmt_dt(b.start), "end": fmt_dt(b.end), "title": b.title,
         "owner": b.owner, "attendees": b.attendees}
#@A
    d["tz"] = b.tz
#@E
    return d


def wait_dict(e):
    d = {"id": e.id, "room": e.room, "start": fmt_dt(e.start), "end": fmt_dt(e.end), "title": e.title,
         "owner": e.owner, "attendees": e.attendees}
#@A
    d["tz"] = e.tz
#@E
    return d


def wait_cells(e):
#@B
    return [e.id, e.room, fmt_dt(e.start), fmt_dt(e.end), e.title, e.owner, e.attendees]
#@A
    return [e.id, e.room, fmt_dt(e.start), fmt_dt(e.end), e.title, e.owner, e.attendees, e.tz]
#@E


def history_dict(h):
    d = {"seq": h.seq, "action": h.action, "booking": h.booking, "room": h.room, "start": fmt_dt(h.start),
         "end": fmt_dt(h.end)}
#@A
    d["tz"] = h.tz
#@E
    return d


def history_cells(h):
    d = history_dict(h)
    return [d[k] for k in HISTORY_HEADERS]


def room_cells(r):
#@B
    return [r.id, r.name, r.capacity, r.hours_label(), r.tags]
#@A
    return [r.id, r.name, r.capacity, r.hours_label(), r.tags, r.tz]
#@E


def booking_cells(b):
#@B
    return [b.id, b.room, fmt_dt(b.start), fmt_dt(b.end), b.title, b.owner, b.attendees]
#@A
    return [b.id, b.room, fmt_dt(b.start), fmt_dt(b.end), b.title, b.owner, b.attendees, b.tz]
#@E


def emit_json(obj):
    sys.stdout.write(json.dumps(obj, indent=2) + "\n")


def say(text):
    sys.stdout.write(text + "\n")


def table(headers, rows, empty="(none)"):
    sys.stdout.write(render(list(headers), rows, empty))


def rows_table(rows, empty="(none)"):
    """A list of dict rows (reports) as a table, the keys of the first row as headers."""
    if not rows:
        say(empty)
        return
    headers = list(rows[0].keys())
    table(headers, [[r.get(h) for h in headers] for r in rows])
