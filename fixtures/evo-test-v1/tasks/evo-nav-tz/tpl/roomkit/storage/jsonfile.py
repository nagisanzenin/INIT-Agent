"""The JSON-file store: one document {"schema": N, "rooms", "bookings", "waitlist", "history"} (migrate.py)."""
import json
import os

from roomkit.errors import ParseError, StoreError, ValidationError
from roomkit.models.calendar import Calendar
from roomkit.models.serialize import booking_from_record, history_from_record, room_from_record, wait_from_record
from roomkit.storage.base import Store
from roomkit.storage.migrate import JSON_VERSION, upgrade_json
from roomkit.util.text import join_tags
from roomkit.util.timefmt import fmt_dt, fmt_hhmm


def room_to_json(r):
    return {
        "id": r.id,
        "name": r.name,
        "capacity": r.capacity,
        "opens": fmt_hhmm(r.opens),
        "closes": fmt_hhmm(r.closes),
        "tags": list(r.tags),
#@A
        "tz": r.tz,
#@E
    }


def booking_to_json(b):
    return {
        "id": b.id,
        "room": b.room,
        "start": fmt_dt(b.start),
        "end": fmt_dt(b.end),
        "title": b.title,
        "owner": b.owner,
        "attendees": b.attendees,
#@A
        "tz": b.tz,
#@E
    }


def wait_to_json(e):
    out = {"id": e.id, "room": e.room, "start": fmt_dt(e.start), "end": fmt_dt(e.end), "title": e.title,
           "owner": e.owner, "attendees": e.attendees}
#@A
    out["tz"] = e.tz
#@E
    return out


def history_to_json(h):
    out = {"seq": h.seq, "action": h.action, "booking": h.booking, "room": h.room, "start": fmt_dt(h.start),
           "end": fmt_dt(h.end)}
#@A
    out["tz"] = h.tz
#@E
    return out


def _record(d):
    """A JSON object -> a record of strings (tags joined, None as '')."""
    rec = {}
    for k, v in d.items():
        if k == "tags" and isinstance(v, list):
            rec[k] = join_tags([str(x) for x in v])
        elif v is None:
            rec[k] = ""
        else:
            rec[k] = str(v)
    return rec


class JsonFileStore(Store):
    kind = "json"

    def load(self):
        name = os.path.basename(self.path)
        if not os.path.exists(self.path):
            return Calendar()
        try:
            with open(self.path, "r", encoding="utf-8") as f:
                doc = json.load(f)
        except OSError as e:
            raise StoreError("%s: %s" % (name, e.strerror or e))
        except ValueError as e:
            raise StoreError("%s: bad JSON (%s)" % (name, e))
        if not isinstance(doc, dict):
            raise StoreError("%s: expected a JSON object" % name)
        doc = upgrade_json(doc, name)
        cal = Calendar()
        for i, r in enumerate(doc["rooms"], 1):
            try:
                cal.add_room(room_from_record(_record(r)))
            except (ParseError, ValidationError) as e:
                raise StoreError(e.message).at("%s room %d" % (name, i))
        for i, b in enumerate(doc["bookings"], 1):
            try:
                cal.add_booking(booking_from_record(_record(b)))
            except (ParseError, ValidationError) as e:
                raise StoreError(e.message).at("%s booking %d" % (name, i))
        for i, w in enumerate(doc["waitlist"], 1):
            try:
                cal.add_wait(wait_from_record(_record(w)))
            except (ParseError, ValidationError) as e:
                raise StoreError(e.message).at("%s waitlist entry %d" % (name, i))
        for i, h in enumerate(doc["history"], 1):
            try:
                cal.history.append(history_from_record(_record(h)))
            except ParseError as e:
                raise StoreError(e.message).at("%s history entry %d" % (name, i))
        return cal

    def save(self, cal):
        doc = {"schema": JSON_VERSION,
               "rooms": [room_to_json(r) for r in cal.rooms_sorted()],
               "bookings": [booking_to_json(b) for b in cal.bookings_sorted()],
               "waitlist": [wait_to_json(e) for e in cal.waitlist_sorted()],
               "history": [history_to_json(h) for h in cal.history]}
        d = os.path.dirname(self.path)
        try:
            if d:
                os.makedirs(d, exist_ok=True)
            with open(self.path, "w", encoding="utf-8", newline="\n") as f:
                f.write(json.dumps(doc, indent=2) + "\n")
        except OSError as e:
            raise StoreError("%s: %s" % (os.path.basename(self.path), e.strerror or e))
