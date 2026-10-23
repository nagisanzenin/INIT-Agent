"""The owner digest: one owner's bookings grouped by the day they start on (`roomkit digest`,
GET /owners/NAME/digest). Not a registry report: it is keyed by owner, not by options."""
from roomkit.util.timefmt import fmt_date, span_label
#@A
from roomkit.util import zones
#@E


#@B
def digest(cal, owner, first, last):
#@A
def digest(cal, owner, first, last, tz=None):
#@E
    """[{"date", "items": [{"id", "room", "time", "title"}]}] for each day from `first` to `last` on which a booking of
    `owner` starts, sorted by date; a day's items are sorted by start, then id."""
#@A
    zone = "UTC" if tz is None else zones.parse_zone(tz)
#@E
    days = {}
    for b in cal.bookings_sorted():
        if b.owner != owner:
            continue
#@B
        start, end = b.start, b.end
        key = (b.start, b.id)
#@A
        start, end = b.in_zone(zone)
        key = (b.utc_start(), b.id)
#@E
        day = start.date()
        if first <= day <= last:
            item = {"id": b.id, "room": b.room, "time": span_label(start, end), "title": b.title}
            days.setdefault(day, []).append((key, item))
    return [{"date": fmt_date(d), "items": [item for _, item in sorted(days[d], key=lambda x: x[0])]}
            for d in sorted(days)]


def digest_lines(days):
    """The text form: the date, then one indented line per booking ('  09:00-10:00 atlas Standup')."""
    out = []
    for day in days:
        out.append(day["date"])
        for item in day["items"]:
            out.append("  %s %s %s" % (item["time"], item["room"], item["title"]))
    return out
