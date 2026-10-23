"""A booking of one room for one time span."""
from dataclasses import dataclass, replace
from datetime import datetime
from typing import Optional

from roomkit.util.timefmt import fmt_dt, minutes_between, span_label
#@A
from roomkit.util import zones
#@E


@dataclass
class Booking:
    id: Optional[int]
    room: str
    start: datetime
    end: datetime
    title: str
    owner: str
    attendees: int = 1
#@A
    tz: str = "UTC"
#@E

    @property
    def minutes(self):
        return minutes_between(self.start, self.end)

    def day(self):
        """The date the booking starts on (in its own zone)."""
        return self.start.date()

#@A
    def utc_start(self):
        return zones.to_utc(self.start, self.tz)

    def utc_end(self):
        return zones.to_utc(self.end, self.tz)

    def in_zone(self, label):
        """(start, end) converted to the zone `label`."""
        return zones.convert(self.start, self.tz, label), zones.convert(self.end, self.tz, label)

#@E
    def overlaps(self, other):
        """True when both spans share at least one minute (back to back does not overlap)."""
#@B
        return self.start < other.end and other.start < self.end
#@A
        return self.utc_start() < other.utc_end() and other.utc_start() < self.utc_end()
#@E

    def span(self):
        """'09:00-10:00'."""
        return span_label(self.start, self.end)

    def describe(self):
        return "#%s %s %s %s (%s)" % (self.id, self.room, fmt_dt(self.start), self.span(), self.title)

    def copy(self, **changes):
        return replace(self, **changes)
