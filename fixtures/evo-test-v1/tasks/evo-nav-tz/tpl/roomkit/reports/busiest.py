"""The busiest-hours report: how many bookings touch each hour of the day."""
from datetime import timedelta

from roomkit.reports.common import in_range, opt_date, opt_text
from roomkit.util.timefmt import floor_hour
#@A
from roomkit.reports.common import opt_zone
#@E

#@B
OPTIONS = ("date_from", "date_to", "room")
#@A
OPTIONS = ("date_from", "date_to", "room", "tz")
#@E
REQUIRED = ()


def hours_touched(start, end):
    """The hours of day h for which [start, end) overlaps [h:00, h+1:00) on some day."""
    hours = set()
    t = floor_hour(start)
    while t < end:
        hours.add(t.hour)
        t += timedelta(hours=1)
    return hours


def busiest_rows(cal, options):
    first = opt_date(options, "date_from")
    last = opt_date(options, "date_to")
    only = opt_text(options, "room")
#@A
    zone = opt_zone(options)
#@E
    counts = {}
    for b in cal.bookings.values():
        if only and b.room != only:
            continue
#@B
        start, end = b.start, b.end
#@A
        start, end = b.in_zone(zone)
#@E
        if not in_range(start.date(), first, last):
            continue
        for h in hours_touched(start, end):
            counts[h] = counts.get(h, 0) + 1
    rows = [{"hour": "%02d" % h, "bookings": n} for h, n in counts.items()]
    rows.sort(key=lambda r: (-r["bookings"], r["hour"]))
    return rows
