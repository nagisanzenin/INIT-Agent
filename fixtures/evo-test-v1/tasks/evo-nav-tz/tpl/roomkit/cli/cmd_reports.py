"""Commands that read: agenda, report, reminders, stats, digest, check."""
import sys

from roomkit.cli.args import arg_date, given
from roomkit.cli.output import emit_json, rows_table, say
from roomkit.plugins.registry import check_options, get_notifier, get_report
from roomkit.reports.digest import digest, digest_lines
from roomkit.reports.reminders import reminder_line
from roomkit.reports.stats import STAT_KEYS, stats
from roomkit.util.table import render_pairs
from roomkit.storage.factory import open_store
from roomkit.util.timefmt import fmt_date
from roomkit.validate.ruleset import run_ruleset
#@A
from roomkit.cli.args import arg_zone
#@E

DATE_OPTIONS = ("date", "date_from", "date_to")


def check_option_values(options):
    """Command-line option values must parse (exit 2 otherwise); dates are normalised."""
    for key in DATE_OPTIONS:
        if key in options:
            options[key] = fmt_date(arg_date(options[key]))
#@A
    if "tz" in options:
        options["tz"] = arg_zone(options["tz"])
#@E
    return options


def run_report(store_path, name, options):
    entry = get_report(name)
    check_options(entry, options)
    return entry.fn(open_store(store_path).load(), options)


def cmd_agenda(args):
#@B
    options = check_option_values(given(date=args.date, room=args.room))
#@A
    options = check_option_values(given(date=args.date, room=args.room, tz=args.tz))
#@E
    rows = run_report(args.store, "agenda", options)
    if args.json:
        emit_json({"date": options["date"], "rows": rows})
    else:
        rows_table(rows)
    return 0


def cmd_report(args):
    options = check_option_values(given(date=args.date, date_from=args.date_from, date_to=args.date_to,
#@B
                                        room=args.room))
#@A
                                        room=args.room, tz=args.tz))
#@E
    rows = run_report(args.store, args.name, options)
    if args.json:
        emit_json({"report": args.name, "rows": rows})
    else:
        rows_table(rows)
    return 0


def cmd_reminders(args):
#@B
    options = check_option_values(given(date=args.date, before=args.before, room=args.room))
#@A
    options = check_option_values(given(date=args.date, before=args.before, room=args.room, tz=args.tz))
#@E
    rows = run_report(args.store, "reminders", options)
    if args.json:
        emit_json({"date": options["date"], "reminders": rows})
    elif not rows:
        say("(none)")
    else:
        for row in rows:
            say(reminder_line(row))
    return 0


def cmd_notify(args):
#@B
    options = check_option_values(given(date=args.date, room=args.room))
#@A
    options = check_option_values(given(date=args.date, room=args.room, tz=args.tz))
#@E
    entry = get_notifier(args.channel)
    check_options(entry, options)
    sys.stdout.write(entry.fn(open_store(args.store).load(), options))
    return 0


def cmd_stats(args):
#@B
    out = stats(open_store(args.store).load())
#@A
    out = stats(open_store(args.store).load(), tz=arg_zone(args.tz))
#@E
    if args.json:
        emit_json(out)
    else:
        sys.stdout.write(render_pairs([(k, "-" if out[k] is None else out[k]) for k in STAT_KEYS]))
    return 0


def cmd_digest(args):
    first, last = arg_date(args.date_from), arg_date(args.date_to)
#@B
    days = digest(open_store(args.store).load(), args.owner, first, last)
#@A
    days = digest(open_store(args.store).load(), args.owner, first, last, tz=arg_zone(args.tz))
#@E
    if args.json:
        emit_json({"owner": args.owner, "days": days})
    elif not days:
        say("(none)")
    else:
        for line in digest_lines(days):
            say(line)
    return 0


def cmd_check(args):
    issues = run_ruleset(open_store(args.store).load(), args.ruleset)
    if args.json:
        emit_json({"ruleset": args.ruleset, "issues": issues})
    elif not issues:
        say("no issues")
    else:
        for i in issues:
            say("%s: %s" % (i["rule"], i["message"]))
    return 1 if issues else 0
