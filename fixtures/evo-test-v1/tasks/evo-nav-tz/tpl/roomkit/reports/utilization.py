"""The utilization report: booked minutes against open minutes per room over a date range."""
from roomkit.reports.common import opt_text, percent
from roomkit.util.timefmt import date_range, parse_date

OPTIONS = ("date_from", "date_to", "room")
REQUIRED = ("date_from", "date_to")


def utilization_rows(cal, options):
    first = parse_date(options["date_from"])
    last = parse_date(options["date_to"])
    days = len(date_range(first, last))
    only = opt_text(options, "room")
    rows = []
    for room in cal.rooms_sorted():
        if only and room.id != only:
            continue
        open_minutes = days * room.open_minutes()
        booked = 0
        for b in cal.bookings_in(room.id):
#@B
            d = b.start.date()
#@A
            d = b.in_zone(room.tz)[0].date()
#@E
            if first <= d <= last:
                booked += b.minutes
        rows.append({"room": room.id, "open_minutes": open_minutes, "booked_minutes": booked,
                     "percent": percent(booked, open_minutes)})
    return rows
