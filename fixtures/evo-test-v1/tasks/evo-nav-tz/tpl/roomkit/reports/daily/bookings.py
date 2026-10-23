"""Daily metric daily_bookings: how many bookings start on the day."""
from roomkit.reports.daily.base import metric_rows

#@B
OPTIONS = ("date_from", "date_to", "room")
#@A
OPTIONS = ("date_from", "date_to", "room", "tz")
#@E
REQUIRED = ("date_from", "date_to")


def value(day, items):
    return len(items)


def rows(cal, options):
    return metric_rows(cal, options, value)
