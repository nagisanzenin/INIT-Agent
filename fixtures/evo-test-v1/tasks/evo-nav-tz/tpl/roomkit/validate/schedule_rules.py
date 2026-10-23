"""Schedule-wide rules: they look at the whole calendar, not one booking.

Each rule takes a Calendar and returns issues {"rule", "bookings": [ids], "message"} sorted by the booking ids.
"""
from roomkit.util.timefmt import overlap_minutes


def issue(rule, ids, message):
    return {"rule": rule, "bookings": list(ids), "message": message}


def overlap_pairs(bookings):
    """[(a, b, minutes)] for every overlapping pair, a.id < b.id (the caller passes one room's bookings)."""
    items = sorted(bookings, key=lambda b: b.id)
    out = []
    for i, a in enumerate(items):
        for b in items[i + 1:]:
#@B
            m = overlap_minutes(a.start, a.end, b.start, b.end)
#@A
            m = overlap_minutes(a.utc_start(), a.utc_end(), b.utc_start(), b.utc_end())
#@E
            if m > 0:
                out.append((a, b, m))
    return out


def rule_overlaps(cal):
    issues = []
    by_room = {}
    for b in cal.bookings.values():
        by_room.setdefault(b.room, []).append(b)
    for room_id in sorted(by_room):
        for a, b, m in overlap_pairs(by_room[room_id]):
            issues.append(issue("overlaps", (a.id, b.id),
                                "bookings %d and %d overlap in room '%s' (%d min)" % (a.id, b.id, room_id, m)))
    issues.sort(key=lambda x: x["bookings"])
    return issues


#@B
def _clash(a, b):
    return a.start < b.end and b.start < a.end
#@A
def _clash(a, b):
    return a.utc_start() < b.utc_end() and b.utc_start() < a.utc_end()
#@E


def rule_owner_overlap(cal):
    """The same owner in two overlapping bookings of different rooms."""
    items = cal.bookings_sorted()
    issues = []
    for i, a in enumerate(items):
        for b in items[i + 1:]:
            if a.owner == b.owner and a.room != b.room and _clash(a, b):
                issues.append(issue("owner_overlap", (a.id, b.id),
                                    "owner '%s' double-booked: bookings %d and %d" % (a.owner, a.id, b.id)))
    return issues


def busiest_room(cal):
    """The room id with the most bookings (ties: smallest id); None for an empty calendar."""
    counts = {}
    for b in cal.bookings.values():
        counts[b.room] = counts.get(b.room, 0) + 1
    if not counts:
        return None
    return sorted(counts.items(), key=lambda kv: (-kv[1], kv[0]))[0][0]
