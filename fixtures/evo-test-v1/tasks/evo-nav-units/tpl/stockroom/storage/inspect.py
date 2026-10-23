"""What a store holds on disk, without loading it: its kind, the schema version of each file, record counts.
Used by `stockroom info` and `stockroom upgrade`."""
import csv
import json
import os

from ..errors import StorageError
from .json_store import JsonStore
from .schema import CURRENT, HEADER_RE


def _csv_file(path, filename):
    if not os.path.isfile(path):
        return None
    with open(path, encoding="utf-8", newline="") as f:
        m = HEADER_RE.match(f.readline().rstrip("\r\n"))
        rows = sum(1 for row in csv.reader(f) if row) - 1
    return {"file": filename, "version": int(m.group(2)) if m else None, "current": CURRENT[filename],
            "records": max(rows, 0)}


def describe_files(store) -> dict:
    files = []
    if isinstance(store, JsonStore):
        if os.path.isfile(store.path):
            try:
                with open(store.path, encoding="utf-8") as f:
                    data = json.load(f)
            except ValueError as e:
                raise StorageError("inventory.json: not valid JSON (%s)" % e)
            if not isinstance(data, dict):
                raise StorageError("inventory.json: the document must be an object")
            items = data.get("items") or []
            files.append({"file": "inventory.json", "version": data.get("schema", 0),
                          "current": CURRENT["inventory.json"], "records": len(items)})
    else:
        for filename in ("items.csv", "movements.csv", "kits.csv", "packs.csv"):
            entry = _csv_file(os.path.join(store.path, filename), filename)
            if entry is not None:
                files.append(entry)
    return {"kind": store.kind, "path": store.path, "files": files}


def outdated(info) -> list:
    """The files of `info` written at an older version than the current one."""
    return [f for f in info["files"] if isinstance(f["version"], int) and f["version"] < f["current"]]
