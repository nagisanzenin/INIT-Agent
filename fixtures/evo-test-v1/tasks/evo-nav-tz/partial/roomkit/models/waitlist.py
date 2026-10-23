"""Waitlist entries: requests for a room at a time when it is busy. They are booked automatically, in id order,
as soon as the time becomes free (services/waitlist.py)."""
from dataclasses import dataclass, replace
from datetime import datetime

from roomkit.models.booking import Booking
from roomkit.util.timefmt import minutes_between


@dataclass
class WaitEntry:
    id: int
    room: str
    start: datetime
    end: datetime
    title: str
    owner: str
    attendees: int = 1
    tz: str = "UTC"

    @property
    def minutes(self):
        return minutes_between(self.start, self.end)

    def as_booking(self, booking_id=None):
        """The booking this entry asks for (no id until it is booked)."""
        return Booking(booking_id, self.room, self.start, self.end, self.title, self.owner, self.attendees,
                       tz=self.tz)

    def copy(self, **changes):
        return replace(self, **changes)
