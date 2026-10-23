"""Schema versions of the files the stores write.

Each kind of file carries its own version: the CSV files in their first line ("#stockroom-items v1",
"#stockroom-kits v1", ...), inventory.json in its "schema" key (a file without the key is version 0). A loader
accepts the versions in READABLE and upgrades older ones record by record (storage.migrate); a save always
writes the CURRENT version.
"""
import re

from ..errors import StorageError

ITEMS_CSV = 2
MOVEMENTS_CSV = 2
KITS_CSV = 2
PACKS_CSV = 2
INVENTORY_JSON = 2

CURRENT = {
    "items.csv": ITEMS_CSV,
    "movements.csv": MOVEMENTS_CSV,
    "kits.csv": KITS_CSV,
    "packs.csv": PACKS_CSV,
    "inventory.json": INVENTORY_JSON,
}

READABLE = {
    "items.csv": (1, 2),
    "movements.csv": (1, 2),
    "kits.csv": (1, 2),
    "packs.csv": (1, 2),
    "inventory.json": (0, 1, 2),
}

HEADER_RE = re.compile(r"^#stockroom-(items|movements|kits|packs) v([0-9]+)$")


def csv_header(filename) -> str:
    """The first line of a CSV store file at the current version."""
    return "#stockroom-%s v%d" % (filename.split(".")[0], CURRENT[filename])


def check_version(filename, version) -> int:
    if isinstance(version, bool) or not isinstance(version, int) or version not in READABLE[filename]:
        raise StorageError("%s: unsupported schema version %s" % (filename, version))
    return version


def parse_csv_header(line, filename) -> int:
    """The version a CSV store file declares in its first line."""
    m = HEADER_RE.match(line.rstrip("\r\n"))
    if not m or "%s.csv" % m.group(1) != filename:
        raise StorageError("%s: not a stockroom file (bad first line)" % filename)
    return check_version(filename, int(m.group(2)))
