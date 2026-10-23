"""The audit history: one entry per change to a booking, with the booking's room and times as they were after the
change (for a cancel: as they were when it was cancelled)."""
from dataclasses import dataclass
from datetime import datetime

ACTIONS = ("book", "cancel", "move", "import", "waitlist")


@dataclass
class HistoryEntry:
    seq: int
    action: str
    booking: int
    room: str
    start: datetime
    end: datetime
#@A
    tz: str = "UTC"
#@E


def entry_for(seq, action, b):
    """The history entry for `action` applied to booking `b`."""
#@B
    return HistoryEntry(seq, action, b.id, b.room, b.start, b.end)
#@A
    return HistoryEntry(seq, action, b.id, b.room, b.start, b.end, b.tz)
#@E
