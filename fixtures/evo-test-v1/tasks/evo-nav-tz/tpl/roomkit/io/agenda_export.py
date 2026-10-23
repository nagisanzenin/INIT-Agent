"""The agenda exporter (registered as "agenda"): a tab-separated list for people to read.

    date        time         room   title    owner
    2026-03-02  09:00-10:00  atlas  Standup  ana     (tab-separated, one header line)
"""
from roomkit.util.timefmt import fmt_date, span_label
#@A
from roomkit.util.zones import parse_zone
#@E

#@B
OPTIONS = ("room",)
#@A
OPTIONS = ("room", "tz")
#@E
REQUIRED = ()
HEADER = ("date", "time", "room", "title", "owner")


def export_agenda(bookings, options=None):
    options = options or {}
    room = options.get("room")
#@A
    zone = parse_zone(options.get("tz", "UTC"))
#@E
    rows = []
    for b in bookings:
        if room and b.room != room:
            continue
#@B
        start, end = b.start, b.end
        key = (b.start, b.room, b.id)
#@A
        start, end = b.in_zone(zone)
        key = (b.utc_start(), b.room, b.id)
#@E
        rows.append((key, [fmt_date(start.date()), span_label(start, end), b.room, b.title, b.owner]))
    rows.sort(key=lambda r: r[0])
    lines = ["\t".join(HEADER)] + ["\t".join(cells) for _, cells in rows]
    return "\n".join(lines) + "\n"
