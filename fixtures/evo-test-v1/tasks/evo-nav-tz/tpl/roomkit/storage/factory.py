"""open_store(path): a path ending in .json is a JSON-file store, anything else a CSV-directory store."""
from roomkit.storage.csvdir import CsvDirStore
from roomkit.storage.jsonfile import JsonFileStore

KINDS = {"csv": CsvDirStore, "json": JsonFileStore}


def store_kind(path):
    return "json" if str(path).lower().endswith(".json") else "csv"


def open_store(path):
    return KINDS[store_kind(path)](path)


def load_calendar(path):
    """The calendar of the store at `path` (a shortcut used by pipelines and the CLI)."""
    return open_store(path).load()
