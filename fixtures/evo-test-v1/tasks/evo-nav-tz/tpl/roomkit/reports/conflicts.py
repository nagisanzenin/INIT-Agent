"""The conflicts report: every overlapping pair of bookings in a room, with the minutes they share."""
from roomkit.reports.common import opt_text
from roomkit.util.timefmt import overlap_minutes

OPTIONS = ("room",)
REQUIRED = ()


def conflict_rows(cal, options):
    only = opt_text(options, "room")
    by_room = {}
    for b in cal.bookings.values():
        by_room.setdefault(b.room, []).append(b)
    rows = []
    for room_id in sorted(by_room):
        if only and room_id != only:
            continue
        items = sorted(by_room[room_id], key=lambda b: b.id)
        for i, a in enumerate(items):
            for b in items[i + 1:]:
#@B
                m = overlap_minutes(a.start, a.end, b.start, b.end)
#@A
                m = overlap_minutes(a.utc_start(), a.utc_end(), b.utc_start(), b.utc_end())
#@E
                if m > 0:
                    rows.append({"room": room_id, "a": a.id, "b": b.id, "overlap_minutes": m})
    return rows
