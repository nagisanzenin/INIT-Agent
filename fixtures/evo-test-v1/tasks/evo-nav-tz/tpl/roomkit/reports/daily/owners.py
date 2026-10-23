"""Daily metric daily_owners: how many different owners have a booking starting on the day."""
from roomkit.reports.daily.base import metric_rows

#@B
OPTIONS = ("date_from", "date_to", "room")
#@A
OPTIONS = ("date_from", "date_to", "room", "tz")
#@E
REQUIRED = ("date_from", "date_to")


def value(day, items):
    return len({i.booking.owner for i in items})


def rows(cal, options):
    return metric_rows(cal, options, value)
