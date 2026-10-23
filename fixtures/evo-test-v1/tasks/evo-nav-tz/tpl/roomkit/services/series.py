"""Recurring bookings: the same time span every N days, booked all together or not at all."""
from datetime import timedelta

from roomkit.errors import RoomkitError, ValidationError
from roomkit.services.booking import book

MAX_COUNT = 52


def occurrences(start, end, count, every_days):
    """[(start, end)] of `count` occurrences, `every_days` apart."""
    step = timedelta(days=every_days)
    return [(start + i * step, end + i * step) for i in range(count)]


#@B
def book_series(cal, room_id, start, end, title, owner, count, every_days=7, attendees=1):
#@A
def book_series(cal, room_id, start, end, title, owner, count, every_days=7, attendees=1, tz=None):
#@E
    """Book every occurrence; if one fails, none is kept and the error names the occurrence
    (`occurrence 3: room 'atlas' is busy: conflicts with booking 7`)."""
    if count < 1 or count > MAX_COUNT:
        raise ValidationError("count must be between 1 and %d" % MAX_COUNT)
    if every_days < 1:
        raise ValidationError("every must be >= 1 day")
    made = []
    mark = cal.history_mark()
    for i, (s, e) in enumerate(occurrences(start, end, count, every_days), 1):
        try:
#@B
            made.append(book(cal, room_id, s, e, title, owner, attendees))
#@A
            made.append(book(cal, room_id, s, e, title, owner, attendees, tz=tz))
#@E
        except RoomkitError as err:
            for b in made:
                cal.remove_booking(b.id)
            cal.history_rollback(mark)
            raise type(err)(err.message).at("occurrence %d" % i)
    return made


def series_ids(bookings):
    """'4, 5, 6'."""
    return ", ".join(str(b.id) for b in bookings)
