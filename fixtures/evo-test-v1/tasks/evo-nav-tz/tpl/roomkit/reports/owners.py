"""The owners report: how many bookings and minutes each owner holds."""
from roomkit.reports.common import opt_text

OPTIONS = ("room",)
REQUIRED = ()


def owner_rows(cal, options):
    """Rows {"owner", "bookings", "minutes"} sorted by minutes descending, then owner."""
    only = opt_text(options, "room")
    totals = {}
    for b in cal.bookings.values():
        if only and b.room != only:
            continue
        t = totals.setdefault(b.owner, [0, 0])
        t[0] += 1
        t[1] += b.minutes
    rows = [{"owner": o, "bookings": n, "minutes": m} for o, (n, m) in totals.items()]
    rows.sort(key=lambda r: (-r["minutes"], r["owner"]))
    return rows
