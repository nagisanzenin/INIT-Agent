"""The agenda report: the bookings starting on one date."""
from roomkit.reports.common import opt_text
from roomkit.util.timefmt import end_label, fmt_hhmm, parse_date
#@A
from roomkit.reports.common import opt_zone
#@E

#@B
OPTIONS = ("date", "room")
#@A
OPTIONS = ("date", "room", "tz")
#@E
REQUIRED = ("date",)


def agenda_rows(cal, options):
    day = parse_date(options["date"])
    room = opt_text(options, "room")
#@A
    zone = opt_zone(options)
#@E
    rows = []
    for b in cal.bookings.values():
        if room and b.room != room:
            continue
#@B
        start, end = b.start, b.end
        key = (b.start, b.room, b.id)
#@A
        start, end = b.in_zone(zone)
        key = (b.utc_start(), b.room, b.id)
#@E
        if start.date() != day:
            continue
        rows.append((key, {"id": b.id, "room": b.room, "start": fmt_hhmm(start), "end": end_label(start, end),
                           "title": b.title, "owner": b.owner}))
    rows.sort(key=lambda r: r[0])
    return [row for _, row in rows]
