"""Pipeline sinks: an export (any registered exporter) or a report (any registered report) written to a file."""
import json
import os

from roomkit.errors import StoreError
from roomkit.models.calendar import Calendar
from roomkit.plugins.config import text_options
from roomkit.plugins.registry import check_options, get_exporter, get_report


def write_text(path, text):
    d = os.path.dirname(path)
    try:
        if d:
            os.makedirs(d, exist_ok=True)
        with open(path, "w", encoding="utf-8", newline="") as f:
            f.write(text)
    except OSError as e:
        raise StoreError("%s: %s" % (path, e.strerror or e))


def write_output(spec, bookings):
    """Export the bookings with the named exporter; returns how many bookings were written."""
    entry = get_exporter(spec["format"])
    options = text_options(spec.get("options"))
    check_options(entry, options)
    write_text(spec["path"], entry.fn(list(bookings), options))
    return len(bookings)


def report_calendar(bookings, rooms_cal):
    """A calendar for a report sink: the pipeline's bookings and the rooms of `rooms_cal` (if any)."""
    rooms = [r.copy() for r in rooms_cal.rooms_sorted()] if rooms_cal is not None else []
    cal = Calendar(rooms)
    for b in bookings:
        cal.bookings[b.id] = b.copy()
    return cal


def write_report(spec, bookings, rooms_cal):
    """Run the named report over the bookings; writes {"report", "rows"}; returns the number of rows."""
    entry = get_report(spec["name"])
    options = text_options(spec.get("options"))
    check_options(entry, options)
    rows = entry.fn(report_calendar(bookings, rooms_cal), options)
    write_text(spec["path"], json.dumps({"report": entry.name, "rows": rows}, indent=2) + "\n")
    return len(rows)
