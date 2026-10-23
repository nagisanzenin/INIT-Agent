"""Daily metric daily_longest: the minutes of the longest booking starting on the day (0 when there is none)."""
from roomkit.reports.daily.base import metric_rows

#@B
OPTIONS = ("date_from", "date_to", "room")
#@A
OPTIONS = ("date_from", "date_to", "room", "tz")
#@E
REQUIRED = ("date_from", "date_to")


def value(day, items):
    return max([i.minutes for i in items] or [0])


def rows(cal, options):
    return metric_rows(cal, options, value)
