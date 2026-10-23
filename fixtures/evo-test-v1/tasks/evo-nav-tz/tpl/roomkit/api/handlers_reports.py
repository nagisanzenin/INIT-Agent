"""Handlers for /agenda, /reports/NAME and /import/FORMAT."""
from roomkit.api.http import check_keys, need
from roomkit.api.serialize import history_json
from roomkit.plugins.registry import check_options, get_importer, get_notifier, get_report
from roomkit.reports.digest import digest
from roomkit.reports.stats import stats
from roomkit.services.merge import merge_drafts
from roomkit.util.timefmt import fmt_date, parse_date

#@B
AGENDA_PARAMS = ("date", "room")
#@A
AGENDA_PARAMS = ("date", "room", "tz")
#@E
IMPORT_PARAMS = ("policy",)
#@B
STATS_PARAMS = ()
#@A
STATS_PARAMS = ("tz",)
#@E
#@B
DIGEST_PARAMS = ("date_from", "date_to")
#@A
DIGEST_PARAMS = ("date_from", "date_to", "tz")
#@E


def _rows(ctx, name, options):
    entry = get_report(name)
    check_options(entry, options)
    return entry.fn(ctx.cal, options)


def agenda(ctx, req):
    check_keys(req.query, AGENDA_PARAMS, "parameter")
    day = fmt_date(parse_date(need(req.query, "date")))
    options = dict(req.query)
    options["date"] = day
    return 200, {"date": day, "rows": _rows(ctx, "agenda", options)}


def run_report(ctx, req, name):
    return 200, {"report": name, "rows": _rows(ctx, name, dict(req.query))}


def list_history(ctx, req):
    check_keys(req.query, ("booking",), "parameter")
    raw = req.query.get("booking")
    entries = [h for h in ctx.cal.history if raw in (None, "") or str(h.booking) == raw]
    return 200, {"history": [history_json(h) for h in entries]}


def notify(ctx, req, channel):
    entry = get_notifier(channel)
    options = dict(req.query)
    check_options(entry, options)
    return 200, {"channel": channel, "text": entry.fn(ctx.cal, options)}


def store_stats(ctx, req):
    check_keys(req.query, STATS_PARAMS, "parameter")
#@B
    return 200, stats(ctx.cal)
#@A
    return 200, stats(ctx.cal, tz=req.query.get("tz"))
#@E


def owner_digest(ctx, req, owner):
    check_keys(req.query, DIGEST_PARAMS, "parameter")
    first = parse_date(need(req.query, "date_from"))
    last = parse_date(need(req.query, "date_to"))
#@B
    return 200, {"owner": owner, "days": digest(ctx.cal, owner, first, last)}
#@A
    return 200, {"owner": owner, "days": digest(ctx.cal, owner, first, last, tz=req.query.get("tz"))}
#@E


def import_text(ctx, req, fmt):
    check_keys(req.query, IMPORT_PARAMS, "parameter")
    entry = get_importer(fmt)
    drafts = entry.fn(req.text())
    result = merge_drafts(ctx.cal, drafts, req.query.get("policy", "skip"))
    if result["added"]:
        ctx.touch()
    return 200, result
