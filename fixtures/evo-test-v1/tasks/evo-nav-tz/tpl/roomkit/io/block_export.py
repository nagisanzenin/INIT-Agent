"""The block exporter: the format block_import reads, one block per booking in the order given."""
from roomkit.util.timefmt import fmt_dt

OPTIONS = ()
REQUIRED = ()


def block_lines(b):
    lines = [
        "BEGIN:BOOKING",
        "ID:%d" % b.id,
        "ROOM:%s" % b.room,
        "START:%s" % fmt_dt(b.start),
        "END:%s" % fmt_dt(b.end),
    ]
#@A
    lines.append("TZ:%s" % b.tz)
#@E
    lines += [
        "TITLE:%s" % b.title,
        "OWNER:%s" % b.owner,
        "ATTENDEES:%d" % b.attendees,
        "END:BOOKING",
    ]
    return lines


def export_blocks(bookings, options=None):
    blocks = ["\n".join(block_lines(b)) for b in bookings]
    return "\n\n".join(blocks) + "\n" if blocks else ""
