"""Schema versions of the store files and the upgrades between them.

The CSV directory and the JSON file have their own version numbers (CSV_VERSION, JSON_VERSION). A file is read at
its own version and upgraded step by step to the newest one; saving always writes the newest. Each CSV file is
upgraded on its own (rooms.csv and bookings.csv may be at different versions).

CSV history:
    1  rooms    id,name,capacity,hours            (hours "08:00-18:00", no tags)
       bookings id,room,date,from,to,title,owner  (no attendees)
    2  rooms    id,name,capacity,opens,closes,tags
       bookings id,room,start,end,title,owner,attendees
#@A
    3  rooms    + tz          (upgraded rooms get "UTC")
       bookings + tz          (upgraded bookings get their room's zone, "UTC" when the room is unknown)
#@E

JSON history: version 1 used the key "version" and the version-1 fields above; version 2 is
{"schema": 2, "rooms": [...], "bookings": [...], "waitlist": [...]} with typed values (capacity, id, attendees ints;
tags a list); waitlist entries look like bookings (a version-1 document has no waitlist).
#@A
Version 3 adds "tz" to rooms and bookings.
#@E
"""
from roomkit.errors import StoreError

#@B
CSV_VERSION = 2
JSON_VERSION = 2
#@A
CSV_VERSION = 3
JSON_VERSION = 3
#@E


def check_version(version, newest, name):
    if not isinstance(version, int) or isinstance(version, bool) or version < 1:
        raise StoreError("%s: bad schema version %r" % (name, version))
    if version > newest:
        raise StoreError("%s: unsupported schema version %d (newest is %d)" % (name, version, newest))


def split_hours(text):
    """'08:00-18:00' -> ('08:00', '18:00'); anything else -> (text, '') so the field rules report it."""
    parts = (text or "").split("-")
    if len(parts) == 2:
        return parts[0].strip(), parts[1].strip()
    return text or "", ""


# CSV rows --------------------------------------------------------------------------------------------------
def room_v1_to_v2(rec):
    opens, closes = split_hours(rec.get("hours", ""))
    return {"id": rec.get("id", ""), "name": rec.get("name", ""), "capacity": rec.get("capacity", ""),
            "opens": opens, "closes": closes, "tags": ""}


def booking_v1_to_v2(rec):
    day = rec.get("date", "")
    return {"id": rec.get("id", ""), "room": rec.get("room", ""),
            "start": "%s %s" % (day, rec.get("from", "")), "end": "%s %s" % (day, rec.get("to", "")),
            "title": rec.get("title", ""), "owner": rec.get("owner", ""), "attendees": "1"}


#@A
def room_v2_to_v3(rec):
    out = dict(rec)
    out["tz"] = "UTC"
    return out


def booking_v2_to_v3(rec, room_zones):
    out = dict(rec)
    out["tz"] = room_zones.get(rec.get("room", ""), "UTC")
    return out


#@E
def upgrade_room_rows(version, rows, name):
    """[(line, record)] at `version` -> the same rows at CSV_VERSION."""
    check_version(version, CSV_VERSION, name)
    if version == 1:
        rows = [(n, room_v1_to_v2(r)) for n, r in rows]
        version = 2
#@A
    if version == 2:
        rows = [(n, room_v2_to_v3(r)) for n, r in rows]
        version = 3
#@E
    return rows


#@B
def upgrade_booking_rows(version, rows, name):
#@A
def upgrade_booking_rows(version, rows, name, room_zones=None):
#@E
    """[(line, record)] at `version` -> the same rows at CSV_VERSION."""
    check_version(version, CSV_VERSION, name)
    if version == 1:
        rows = [(n, booking_v1_to_v2(r)) for n, r in rows]
        version = 2
#@A
    if version == 2:
        zones = room_zones or {}
        rows = [(n, booking_v2_to_v3(r, zones)) for n, r in rows]
        version = 3
#@E
    return rows


def upgrade_history_rows(version, rows, name, room_zones=None):
    """history.csv rows: the columns did not change from version 1 to 2 (the file came with version 2; a file
    without a schema line is read as version 1 with the same columns)."""
    check_version(version, CSV_VERSION, name)
#@A
    if version < 3:
        zones = room_zones or {}
        rows = [(n, dict(r, tz=zones.get(r.get("room", ""), "UTC"))) for n, r in rows]
#@E
    return rows


# JSON documents ----------------------------------------------------------------------------------------------
def json_version(doc, name):
    if "schema" in doc:
        return doc["schema"]
    v = doc.get("version", 1)
    if v != 1:
        raise StoreError("%s: a document without \"schema\" must be version 1" % name)
    return 1


def json_room_v1_to_v2(r):
    opens, closes = split_hours(r.get("hours", ""))
    return {"id": r.get("id"), "name": r.get("name"), "capacity": r.get("capacity"),
            "opens": opens, "closes": closes, "tags": []}


def json_booking_v1_to_v2(b):
    day = b.get("date", "")
    return {"id": b.get("id"), "room": b.get("room"), "start": "%s %s" % (day, b.get("from", "")),
            "end": "%s %s" % (day, b.get("to", "")), "title": b.get("title"), "owner": b.get("owner"),
            "attendees": 1}


def upgrade_json(doc, name):
    """A parsed JSON document at any known version -> {"schema": JSON_VERSION, "rooms", "bookings", "waitlist"}."""
    version = json_version(doc, name)
    check_version(version, JSON_VERSION, name)
    rooms = [r for r in (doc.get("rooms") or []) if isinstance(r, dict)]
    bookings = [b for b in (doc.get("bookings") or []) if isinstance(b, dict)]
    waitlist = [w for w in (doc.get("waitlist") or []) if isinstance(w, dict)]
    history = [h for h in (doc.get("history") or []) if isinstance(h, dict)]
    if version == 1:
        rooms = [json_room_v1_to_v2(r) for r in rooms]
        bookings = [json_booking_v1_to_v2(b) for b in bookings]
        version = 2
#@A
    if version == 2:
        rooms = [dict(r, tz="UTC") for r in rooms]
        zones = {r.get("id"): r["tz"] for r in rooms}
        bookings = [dict(b, tz=zones.get(b.get("room"), "UTC")) for b in bookings]
        waitlist = [dict(w, tz=zones.get(w.get("room"), "UTC")) for w in waitlist]
        history = [dict(h, tz=zones.get(h.get("room"), "UTC")) for h in history]
        version = 3
#@E
    return {"schema": version, "rooms": rooms, "bookings": bookings, "waitlist": waitlist, "history": history}
