"""A bookable room with opening hours."""
from dataclasses import dataclass, field, replace
from datetime import time
from typing import List

from roomkit.util.timefmt import fmt_hhmm, minute_of_day

DEFAULT_OPENS = time(8, 0)
DEFAULT_CLOSES = time(18, 0)


@dataclass
class Room:
    id: str
    name: str
    capacity: int
    opens: time = DEFAULT_OPENS
    closes: time = DEFAULT_CLOSES
    tags: List[str] = field(default_factory=list)
#@A
    tz: str = "UTC"
#@E

    def hours_label(self):
        """'08:00-18:00'."""
        return "%s-%s" % (fmt_hhmm(self.opens), fmt_hhmm(self.closes))

    def open_minutes(self):
        """Minutes the room is open on one day."""
        return minute_of_day(self.closes) - minute_of_day(self.opens)

    def is_open_between(self, start_t, end_t):
        """True when [start_t, end_t] (clock times) lies inside the opening hours."""
        return self.opens <= start_t and end_t <= self.closes

    def has_tag(self, tag):
        return tag in self.tags

    def fits(self, attendees):
        return attendees <= self.capacity

    def copy(self, **changes):
        return replace(self, tags=list(self.tags), **changes)
