"""Grouping for the daily metrics: bookings by the day they start on, over an inclusive date range.

Every day of the range gets a row, also days without bookings (the metric decides the value of an empty day).
"""
from roomkit.errors import ValidationError
from roomkit.reports.common import opt_text
from roomkit.util.timefmt import date_range, fmt_date, parse_date
#@A
from roomkit.reports.common import opt_zone
#@E


class DayItem:
    """One booking as a daily metric sees it: the booking and its start and end as the metric shows them."""

    def __init__(self, booking, start, end):
        self.booking = booking
        self.start = start
        self.end = end

    @property
    def minutes(self):
        return self.booking.minutes


def days_of(cal, options):
    """{date: [DayItem]} for every date of the range, items in booking id order."""
    first = parse_date(options["date_from"])
    last = parse_date(options["date_to"])
    if last < first:
        raise ValidationError("date_to is before date_from")
    only = opt_text(options, "room")
#@A
    zone = opt_zone(options)
#@E
    days = {d: [] for d in date_range(first, last)}
    for b in cal.bookings_sorted():
        if only and b.room != only:
            continue
#@B
        start, end = b.start, b.end
#@A
        start, end = b.in_zone(zone)
#@E
        if start.date() in days:
            days[start.date()].append(DayItem(b, start, end))
    return days


def metric_rows(cal, options, value_of):
    """Rows {"date": "YYYY-MM-DD", "value": value_of(day, items)} sorted by date."""
    days = days_of(cal, options)
    return [{"date": fmt_date(d), "value": value_of(d, days[d])} for d in sorted(days)]
