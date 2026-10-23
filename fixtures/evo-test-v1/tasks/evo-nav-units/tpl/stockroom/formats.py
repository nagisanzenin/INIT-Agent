"""File-name extensions and the importer/exporter names they imply (when no --format is given)."""
import os

EXTENSIONS = {
    ".csv": "csv",
    ".tsv": "tsv",
    ".json": "json",
    ".jsonl": "jsonl",
    ".ledger": "ledger",
    ".txt": "ledger",
    ".md": "markdown",
}

CONTENT_TYPES = {
    "csv": "text/csv",
    "tsv": "text/tab-separated-values",
    "json": "application/json",
    "jsonl": "application/x-ndjson",
    "ledger": "text/plain",
    "labels": "text/plain",
    "shop": "text/csv",
    "snapshot": "text/csv",
    "markdown": "text/markdown",
}


def guess_format(path, default=None):
    """The format name for `path` from its extension, or `default`."""
    ext = os.path.splitext(path)[1].lower()
    return EXTENSIONS.get(ext, default)


def format_options(config, fmt):
    """The options a config gives to the importer/exporter named `fmt` (csv and tsv share the csv section)."""
    section = "csv" if fmt in ("csv", "tsv") else fmt
    opts = config.get(section) if isinstance(config, dict) else None
    return dict(opts) if isinstance(opts, dict) else {}
