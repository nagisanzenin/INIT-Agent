"""The reminders report: when to remind each owner of the bookings starting on a date (`roomkit reminders`)."""
from roomkit.errors import ValidationError
from roomkit.reports.common import opt_text
from roomkit.util.timefmt import add_minutes, fmt_dt, fmt_hhmm, parse_date
#@A
from roomkit.reports.common import opt_zone
#@E

#@B
OPTIONS = ("date", "before", "room")
#@A
OPTIONS = ("date", "before", "room", "tz")
#@E
REQUIRED = ("date",)
DEFAULT_BEFORE = 15


def _before(options):
    raw = options.get("before")
    if raw in (None, ""):
        return DEFAULT_BEFORE
    try:
        n = int(raw)
    except ValueError:
        raise ValidationError("bad integer '%s' for before" % raw)
    if n < 0:
        raise ValidationError("before must be >= 0")
    return n


def reminder_rows(cal, options):
    """Rows {"at": "YYYY-MM-DD HH:MM", "id", "owner", "title", "room", "starts": "HH:MM"} for the bookings starting
    on `date`, `before` minutes (default 15) before their start, sorted by start, then id."""
    day = parse_date(options["date"])
    before = _before(options)
    only = opt_text(options, "room")
#@A
    zone = opt_zone(options)
#@E
    rows = []
    for b in cal.bookings_sorted():
        if only and b.room != only:
            continue
#@B
        start = b.start
        key = (b.start, b.id)
#@A
        start = b.in_zone(zone)[0]
        key = (b.utc_start(), b.id)
#@E
        if start.date() != day:
            continue
        rows.append((key, {"at": fmt_dt(add_minutes(start, -before)), "id": b.id, "owner": b.owner,
                           "title": b.title, "room": b.room, "starts": fmt_hhmm(start)}))
    rows.sort(key=lambda r: r[0])
    return [row for _, row in rows]


def reminder_line(row):
    """'2026-03-02 08:45 ana: Standup in atlas at 09:00'."""
    return "%s %s: %s in %s at %s" % (row["at"], row["owner"], row["title"], row["room"], row["starts"])
