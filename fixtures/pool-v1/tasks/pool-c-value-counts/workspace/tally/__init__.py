"""tally: counting helpers for the inventory reports."""
from .counts import value_counts
from .report import format_counts

__all__ = ["value_counts", "format_counts"]
