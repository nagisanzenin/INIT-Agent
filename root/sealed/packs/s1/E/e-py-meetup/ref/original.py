"""Dev fixture: the unmutated exercise's behaviour (first..fifth, last, teenth), returning the date as an
ISO string (the most charitable adaptation to the string interface). It must score below 1000."""
import calendar
import datetime

DAYS = ["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"]
ORDINALS = {"first": 1, "second": 2, "third": 3, "fourth": 4, "fifth": 5}


class MeetupDayException(ValueError):
    def __init__(self, message="That day does not exist."):
        super().__init__(message)
        self.message = message


def meetup(year, month, week, day_of_week):
    wd = DAYS.index(day_of_week)
    ndays = calendar.monthrange(year, month)[1]
    days = [d for d in range(1, ndays + 1) if datetime.date(year, month, d).weekday() == wd]
    if week == "teenth":
        day = [d for d in days if 13 <= d <= 19][0]
    elif week == "last":
        day = days[-1]
    else:
        n = ORDINALS[week]
        if n > len(days):
            raise MeetupDayException("That day does not exist.")
        day = days[n - 1]
    return datetime.date(year, month, day).isoformat()
