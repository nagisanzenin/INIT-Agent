"""Importing rooms from CSV text (`roomkit import-rooms FILE`, POST /import-rooms). Not a registry importer: rooms
are not bookings and are merged by id, not by time.

Columns id, name and capacity are required, the other room columns optional, unknown columns ignored. A bad value
fails the whole import (`line N: <message>`); a room whose id exists already is skipped.
"""
from roomkit.errors import FieldError, ParseError, ValidationError
from roomkit.services.rooms import make_room
from roomkit.util.csvio import parse_rows

REQUIRED_COLUMNS = ("id", "name", "capacity")
#@B
ROOM_COLUMNS = ("id", "name", "capacity", "opens", "closes", "tags")
#@A
ROOM_COLUMNS = ("id", "name", "capacity", "opens", "closes", "tags", "tz")
#@E


def parse_rooms(text):
    """[(where, Room)] in file order."""
    rows = parse_rows(text)
    if not rows:
        raise ParseError("missing header").at("line 1")
    (hn, header), body = rows[0], rows[1:]
    for col in REQUIRED_COLUMNS:
        if col not in header:
            raise ParseError("missing column '%s'" % col).at("line %d" % hn)
    out = []
    for n, cells in body:
        raw = {h: (cells[i] if i < len(cells) else "") for i, h in enumerate(header) if h in ROOM_COLUMNS}
        try:
            out.append(("line %d" % n, make_room(raw)))
        except (FieldError, ValidationError) as e:
            raise ParseError(e.message).at("line %d" % n)
    return out


def merge_rooms(cal, parsed):
    """{"added": [ids], "skipped": [{"where", "reason"}]}."""
    added, skipped = [], []
    for where, room in parsed:
        if cal.has_room(room.id):
            skipped.append({"where": where, "reason": "room '%s' exists" % room.id})
            continue
        cal.add_room(room)
        added.append(room.id)
    return {"added": added, "skipped": skipped}
