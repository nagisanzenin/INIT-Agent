"""Reference solution for e-py-meetup. gen/run replaces the P line with the instance's parameters:
fmt (date format id), window (descriptor name), start (first day of its 7-day window), penult (name of the
second-to-last descriptor or None), int_weeks (bool) and skip (day of month that moves a week later, or 0)."""
import calendar
import datetime

P = None  # @@PARAMS@@

DAYS = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"]
MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
ORDINALS = {"first": 1, "second": 2, "third": 3, "fourth": 4, "fifth": 5}


class MeetupDayException(ValueError):
    """Raised when the requested meetup date does not exist."""

    def __init__(self, message="That day does not exist."):
        super().__init__(message)
        self.message = message


def _fmt(y, m, d):
    f = P["fmt"]
    if f == "iso":
        return "%04d-%02d-%02d" % (y, m, d)
    if f == "dmy_dot":
        return "%02d.%02d.%04d" % (d, m, y)
    if f == "mdy_slash":
        return "%02d/%02d/%04d" % (m, d, y)
    if f == "compact":
        return "%04d%02d%02d" % (y, m, d)
    return "%d %s %04d" % (d, MONTHS[m - 1], y)


def meetup(year, month, week, day_of_week):
    if day_of_week not in DAYS:
        raise MeetupDayException("unknown weekday %r" % (day_of_week,))
    wd = DAYS.index(day_of_week)
    ndays = calendar.monthrange(year, month)[1]
    days = [d for d in range(1, ndays + 1) if datetime.date(year, month, d).weekday() == wd]
    if isinstance(week, int) and not isinstance(week, bool):
        if not P["int_weeks"] or week == 0:
            raise MeetupDayException("week number must not be 0")
        idx = week - 1 if week > 0 else len(days) + week
        if not 0 <= idx < len(days):
            raise MeetupDayException("that day does not exist")
        day = days[idx]
    elif week in ORDINALS:
        n = ORDINALS[week]
        if n > len(days):
            raise MeetupDayException("that day does not exist")
        day = days[n - 1]
    elif week == "last":
        day = days[-1]
    elif P["penult"] is not None and week == P["penult"]:
        day = days[-2]
    elif week == P["window"]:
        day = [d for d in days if P["start"] <= d <= P["start"] + 6][0]
    else:
        raise MeetupDayException("unknown week descriptor %r" % (week,))
    if P["skip"] and day == P["skip"]:
        day += 7
        if day > ndays:
            raise MeetupDayException("the skipped meetup leaves the month")
    return _fmt(year, month, day)
