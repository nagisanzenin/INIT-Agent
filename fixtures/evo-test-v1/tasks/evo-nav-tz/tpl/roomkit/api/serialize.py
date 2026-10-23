"""JSON shapes of rooms and bookings in responses (the same keys as the command line's --json output)."""
from roomkit.util.timefmt import fmt_dt, fmt_hhmm


def room_json(r):
    out = {
        "id": r.id,
        "name": r.name,
        "capacity": r.capacity,
        "opens": fmt_hhmm(r.opens),
        "closes": fmt_hhmm(r.closes),
        "tags": list(r.tags),
    }
#@A
    out["tz"] = r.tz
#@E
    return out


def booking_json(b):
    out = {
        "id": b.id,
        "room": b.room,
        "start": fmt_dt(b.start),
        "end": fmt_dt(b.end),
        "title": b.title,
        "owner": b.owner,
        "attendees": b.attendees,
    }
#@A
    out["tz"] = b.tz
#@E
    return out


def wait_json(e):
    out = {"id": e.id, "room": e.room, "start": fmt_dt(e.start), "end": fmt_dt(e.end), "title": e.title,
           "owner": e.owner, "attendees": e.attendees}
#@A
    out["tz"] = e.tz
#@E
    return out


def history_json(h):
    out = {"seq": h.seq, "action": h.action, "booking": h.booking, "room": h.room, "start": fmt_dt(h.start),
           "end": fmt_dt(h.end)}
#@A
    out["tz"] = h.tz
#@E
    return out


def slots_json(room_id, day, slots):
    return {"room": room_id, "date": "%04d-%02d-%02d" % (day.year, day.month, day.day),
            "slots": [fmt_dt(t) for t in slots]}
