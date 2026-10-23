"""stockroom [--store PATH] [--config FILE] COMMAND ...

Exit status: 0 success; 1 validation problems or rejected import rows; 2 usage errors and every other
StockroomError (printed as "error: <message>").
"""
import argparse
import os

from ..config import load_config
from ..errors import StockroomError, ValidationError
from ..registry.builtin import install_plugins
from ..storage.factory import open_store
from . import commands_admin, commands_io, commands_items, commands_kits, commands_report, commands_stock
from .context import Ctx
from .output import err


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="stockroom", description="A small inventory manager.")
    p.add_argument("--store", help="store directory, or a .json file (default: $STOCKROOM_STORE or config)")
    p.add_argument("--config", help="JSON config file")
    sub = p.add_subparsers(dest="command", metavar="COMMAND")
    sub.required = True
    commands_items.register(sub)
    commands_stock.register(sub)
    commands_io.register(sub)
    commands_report.register(sub)
    commands_admin.register(sub)
    commands_kits.register(sub)
    return p


def main(argv=None) -> int:
    args = build_parser().parse_args(argv)
    try:
        config = load_config(args.config)
        install_plugins(config)
        path = args.store or os.environ.get("STOCKROOM_STORE") or config["store"]
        return args.func(args, Ctx(open_store(path), config)) or 0
    except ValidationError as e:
        for p in e.problems:
            err("invalid: %s" % p)
        return 1
    except StockroomError as e:
        err("error: %s" % e)
        return 2
