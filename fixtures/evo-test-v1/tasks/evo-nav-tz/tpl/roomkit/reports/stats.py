"""The store summary (`roomkit stats`, GET /stats)."""
from roomkit.util.timefmt import fmt_dt
from roomkit.validate.schedule_rules import busiest_room
#@A
from roomkit.util import zones
#@E


#@B
def stats(cal):
#@A
def stats(cal, tz=None):
#@E
    """{"rooms", "bookings", "waitlist", "first_start", "last_end", "busiest_room"}: counts, the earliest start and
    the latest end of all bookings (null without bookings) and the room with the most bookings."""
    items = cal.bookings_sorted()
    out = {"rooms": len(cal.rooms), "bookings": len(items), "waitlist": len(cal.waitlist),
           "first_start": None, "last_end": None, "busiest_room": busiest_room(cal)}
    if items:
#@B
        first = min(items, key=lambda b: (b.start, b.id))
        last = max(items, key=lambda b: (b.end, -b.id))
        out["first_start"] = fmt_dt(first.start)
        out["last_end"] = fmt_dt(last.end)
#@A
        zone = "UTC" if tz is None else zones.parse_zone(tz)
        first = min(items, key=lambda b: (b.utc_start(), b.id))
        last = max(items, key=lambda b: (b.utc_end(), -b.id))
        out["first_start"] = fmt_dt(first.in_zone(zone)[0])
        out["last_end"] = fmt_dt(last.in_zone(zone)[1])
#@E
    return out


STAT_KEYS = ("rooms", "bookings", "waitlist", "first_start", "last_end", "busiest_room")
