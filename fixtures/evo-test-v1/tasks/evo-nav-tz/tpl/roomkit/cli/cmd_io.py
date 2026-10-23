"""Commands that move data: import, import-rooms, history, export, pipeline, migrate."""
from roomkit.cli.args import arg_options
from roomkit.cli.args import arg_booking_id
from roomkit.cli.output import HISTORY_HEADERS, emit_json, history_cells, history_dict, say, table
from roomkit.io.rooms_import import merge_rooms, parse_rooms
from roomkit.pipeline.runner import read_file, run_pipeline_file
from roomkit.pipeline.sinks import write_text
from roomkit.plugins.registry import check_options, get_exporter, get_importer
from roomkit.services.merge import merge_drafts
from roomkit.storage.factory import open_store


def cmd_import(args):
    entry = get_importer(args.format)
    drafts = entry.fn(read_file(args.file))
    result = open_store(args.store).update(lambda cal: merge_drafts(cal, drafts, args.policy))
    if args.json:
        emit_json(result)
    else:
        say("imported %d, skipped %d" % (len(result["added"]), len(result["skipped"])))
        for s in result["skipped"]:
            say("skipped %s: %s" % (s["where"], s["reason"]))
    return 0


def cmd_import_rooms(args):
    parsed = parse_rooms(read_file(args.file))
    result = open_store(args.store).update(lambda cal: merge_rooms(cal, parsed))
    if args.json:
        emit_json(result)
    else:
        say("imported %d rooms, skipped %d" % (len(result["added"]), len(result["skipped"])))
        for s in result["skipped"]:
            say("skipped %s: %s" % (s["where"], s["reason"]))
    return 0


def cmd_history(args):
    entries = open_store(args.store).load().history
    if args.booking is not None:
        bid = arg_booking_id(args.booking)
        entries = [h for h in entries if h.booking == bid]
    if args.json:
        emit_json({"history": [history_dict(h) for h in entries]})
    else:
        table(HISTORY_HEADERS, [history_cells(h) for h in entries])
    return 0


def cmd_export(args):
    entry = get_exporter(args.format)
    options = arg_options(args.opt)
    check_options(entry, options)
    cal = open_store(args.store).load()
    write_text(args.file, entry.fn(cal.bookings_sorted(), options))
    say("exported %d bookings to %s" % (len(cal.bookings), args.file))
    return 0


def cmd_pipeline(args):
    summary = run_pipeline_file(args.config)
    say("pipeline: read %d, wrote %d to %s" % (summary["read"], summary["written"], summary["to"]))
    return 0


def cmd_migrate(args):
    store = open_store(args.store)
    store.save(store.load())
    say("migrated %s" % args.store)
    return 0
