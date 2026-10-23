"""import, export, validate, run."""
import os
import sys

from ..errors import ConfigError, RegistryError, StorageError, StockroomError
from ..exporters.common import select
from ..formats import format_options, guess_format
from ..registry import builtin
from ..registry.pipeline import load_pipeline, run_pipeline
from ..services.merge import MODES, merge
from ..validate.runner import run_rulesets
#@A
from ..units import convert_items, normalize_unit
#@E
from .output import err, out


def register(sub):
    p = sub.add_parser("import", help="import items from a file")
    p.add_argument("file")
    p.add_argument("--format", help="importer name (default: from the file extension)")
    p.add_argument("--mode", choices=MODES, help="add (default) or replace")
    p.set_defaults(func=cmd_import)

    p = sub.add_parser("export", help="export items to a file ('-' for standard output)")
    p.add_argument("file")
    p.add_argument("--format", help="exporter name (default: from the file extension, else the config's)")
    p.add_argument("--category")
#@A
    p.add_argument("--unit", default=None, help="express items of this unit's dimension in it")
#@E
    p.set_defaults(func=cmd_export)

    p = sub.add_parser("validate", help="check the store against the configured rulesets")
    p.set_defaults(func=cmd_validate)

    p = sub.add_parser("run", help="run a pipeline file")
    p.add_argument("pipeline")
    p.set_defaults(func=cmd_run)


def _lookup(kind, name):
    try:
        return builtin.registry(kind).get(name)
    except RegistryError:
        raise StockroomError("unknown %s format: %s" % (kind, name)) from None


def cmd_import(args, ctx):
    fmt = args.format or guess_format(args.file)
    if not fmt:
        raise ConfigError("cannot tell the format of %s (use --format)" % args.file)
    parse = _lookup("importer", fmt)
    try:
        with open(args.file, encoding="utf-8", newline="") as f:
            text = f.read()
    except OSError as e:
        raise StorageError("cannot read %s: %s" % (args.file, e.strerror or e))
    inv = ctx.load()
    result = parse(text, format_options(ctx.config, fmt))
    result.source = os.path.basename(args.file)
    applied = merge(inv, result, args.mode or ctx.config["import"]["mode"])
    if applied:
        ctx.save(inv)
    for line in result.error_lines(args.file):
        err(line)
    out("imported %d item%s, %d error%s" % (applied, "" if applied == 1 else "s", len(result.errors),
                                           "" if len(result.errors) == 1 else "s"))
    return 1 if result.errors else 0


def cmd_export(args, ctx):
    fmt = args.format or (guess_format(args.file) if args.file != "-" else None) or ctx.config["export"]["format"]
    write = _lookup("exporter", fmt)
#@A
    unit = normalize_unit(args.unit) if args.unit else None
#@E
    inv = ctx.load()
    items = select(inv.items.values(), args.category)
#@A
    if unit:
        items = convert_items(items, unit)
#@E
    text = write(items, format_options(ctx.config, fmt))
    if args.file == "-":
        sys.stdout.write(text)
        return 0
    with open(args.file, "w", encoding="utf-8", newline="") as f:
        f.write(text)
    err("exported %d item%s to %s" % (len(items), "" if len(items) == 1 else "s", args.file))
    return 0


def cmd_validate(args, ctx):
    problems = run_rulesets(ctx.load(), ctx.config)
    for p in problems:
        out(str(p))
    if problems:
        err("%d problem%s" % (len(problems), "" if len(problems) == 1 else "s"))
        return 1
    out("ok")
    return 0


def cmd_run(args, ctx):
    config = load_pipeline(args.pipeline)
    base = os.path.dirname(os.path.abspath(args.pipeline))
    res = run_pipeline(config, base, ctx.store)
    for line in res.errors:
        err(line)
    if res.stdout:
        sys.stdout.write(res.stdout)
    err("pipeline: %d in, %d out, %d error%s" % (res.count_in, res.count_out, len(res.errors),
                                                 "" if len(res.errors) == 1 else "s"))
    return 1 if res.errors else 0
