"""Daily metric daily_first_start: the earliest start time on the day ("HH:MM", null when there is none)."""
from roomkit.reports.daily.base import metric_rows
from roomkit.util.timefmt import fmt_hhmm

#@B
OPTIONS = ("date_from", "date_to", "room")
#@A
OPTIONS = ("date_from", "date_to", "room", "tz")
#@E
REQUIRED = ("date_from", "date_to")


def value(day, items):
    return fmt_hhmm(min(i.start for i in items)) if items else None


def rows(cal, options):
    return metric_rows(cal, options, value)
