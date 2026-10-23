"""Daily metric daily_last_end: the latest end ("HH:MM", "HH:MM+1" when it is on a later day, null when there is none)."""
from roomkit.reports.daily.base import metric_rows
from roomkit.util.timefmt import fmt_hhmm

#@B
OPTIONS = ("date_from", "date_to", "room")
#@A
OPTIONS = ("date_from", "date_to", "room", "tz")
#@E
REQUIRED = ("date_from", "date_to")


def _end_label(day, end):
    return fmt_hhmm(end) + ("+1" if end.date() > day else "")


def value(day, items):
    return _end_label(day, max(i.end for i in items)) if items else None


def rows(cal, options):
    return metric_rows(cal, options, value)
