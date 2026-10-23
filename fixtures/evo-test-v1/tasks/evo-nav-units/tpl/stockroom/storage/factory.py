"""open_store(path): the store for a path. A path ending in .json is a JsonStore; anything else is a
CsvStore directory."""
from .csv_store import CsvStore
from .json_store import JsonStore


def open_store(path):
    if str(path).lower().endswith(".json"):
        return JsonStore(path)
    return CsvStore(path)
