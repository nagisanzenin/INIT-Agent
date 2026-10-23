"""The name registry. Importers, exporters, reports and pipeline steps are found by name, as "module:function"
targets, so the command line, the request handler and pipeline configs can name them in plain text.

An entry's options are the module's OPTIONS (accepted) and REQUIRED (needed) tuples; check_options refuses the rest:
    report 'conflicts' does not take option 'x'
    report 'agenda' needs option 'date'
"""
import importlib
from collections import namedtuple

from roomkit.errors import ConfigError
from roomkit.io.calendars import CALENDAR_EXPORTERS
from roomkit.io.feeds import FEED_IMPORTERS
from roomkit.notify import NOTIFIERS
from roomkit.reports.grouped import GROUPED_REPORTS

IMPORTERS = {
    "csv": "roomkit.io.csv_import:parse_csv",
    "json": "roomkit.io.json_import:parse_json",
    "jsonl": "roomkit.io.jsonl_import:parse_jsonl",
    "block": "roomkit.io.block_import:parse_blocks",
    "partner": "roomkit.io.partner_import:parse_partner",
}

EXPORTERS = {
    "csv": "roomkit.io.csv_export:export_csv",
    "json": "roomkit.io.json_export:export_json",
    "block": "roomkit.io.block_export:export_blocks",
    "agenda": "roomkit.io.agenda_export:export_agenda",
}

REPORTS = {
    "agenda": "roomkit.reports.agenda:agenda_rows",
    "utilization": "roomkit.reports.utilization:utilization_rows",
    "conflicts": "roomkit.reports.conflicts:conflict_rows",
    "busiest": "roomkit.reports.busiest:busiest_rows",
    "owners": "roomkit.reports.owners:owner_rows",
    "reminders": "roomkit.reports.reminders:reminder_rows",
    "daily_bookings": "roomkit.reports.daily.bookings:rows",
    "daily_minutes": "roomkit.reports.daily.minutes:rows",
    "daily_attendees": "roomkit.reports.daily.attendees:rows",
    "daily_rooms": "roomkit.reports.daily.rooms:rows",
    "daily_owners": "roomkit.reports.daily.owners:rows",
    "daily_longest": "roomkit.reports.daily.longest:rows",
    "daily_average": "roomkit.reports.daily.average:rows",
    "daily_first_start": "roomkit.reports.daily.first_start:rows",
    "daily_last_end": "roomkit.reports.daily.last_end:rows",
}

STEPS = {
    "where": "roomkit.pipeline.steps:make_where",
    "sort": "roomkit.pipeline.steps:make_sort",
    "limit": "roomkit.pipeline.steps:make_limit",
    "shift": "roomkit.pipeline.steps:make_shift",
    "rename_room": "roomkit.pipeline.steps:make_rename_room",
#@A
    "to_zone": "roomkit.pipeline.steps:make_to_zone",
#@E
}

# the generated families: one partner feed, calendar system or grouped report per module
IMPORTERS.update(FEED_IMPORTERS)
EXPORTERS.update(CALENDAR_EXPORTERS)
REPORTS.update(GROUPED_REPORTS)

TABLES = {"importer": IMPORTERS, "exporter": EXPORTERS, "report": REPORTS, "step": STEPS, "notifier": NOTIFIERS}

Entry = namedtuple("Entry", "kind name fn options required")


def resolve(target):
    """'pkg.module:function' -> the function."""
    module, _, func = target.partition(":")
    try:
        return getattr(importlib.import_module(module), func)
    except (ImportError, AttributeError):
        raise ConfigError("cannot load '%s'" % target)


def get(kind, name):
    table = TABLES[kind]
    if name not in table:
        raise ConfigError("unknown %s '%s'" % (kind, name))
    target = table[name]
    fn = resolve(target)
    module = importlib.import_module(target.partition(":")[0])
    return Entry(kind, name, fn, tuple(getattr(module, "OPTIONS", ())), tuple(getattr(module, "REQUIRED", ())))


def get_importer(name):
    return get("importer", name)


def get_exporter(name):
    return get("exporter", name)


def get_report(name):
    return get("report", name)


def get_step(name):
    return get("step", name)


def get_notifier(name):
    return get("notifier", name)


def check_options(entry, options):
    for key in options:
        if key not in entry.options:
            raise ConfigError("%s '%s' does not take option '%s'" % (entry.kind, entry.name, key))
    for key in entry.required:
        if options.get(key) in (None, ""):
            raise ConfigError("%s '%s' needs option '%s'" % (entry.kind, entry.name, key))


def names(kind):
    return sorted(TABLES[kind])


def register(kind, name, target):
    """Add or replace an entry (used by tests and by code that extends roomkit)."""
    TABLES[kind][name] = target
