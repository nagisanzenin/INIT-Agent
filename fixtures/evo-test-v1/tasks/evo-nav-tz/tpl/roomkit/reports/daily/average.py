"""Daily metric daily_average: the average minutes of the bookings starting on the day, rounded half up to one decimal, as text ("0.0" when there is none)."""
from roomkit.reports.daily.base import metric_rows
from roomkit.reports.common import percent

#@B
OPTIONS = ("date_from", "date_to", "room")
#@A
OPTIONS = ("date_from", "date_to", "room", "tz")
#@E
REQUIRED = ("date_from", "date_to")


def value(day, items):
    return percent(sum(i.minutes for i in items), len(items) * 100) if items else "0.0"


def rows(cal, options):
    return metric_rows(cal, options, value)
