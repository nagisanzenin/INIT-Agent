from .core import rotate_after


def rotate_all(items, mark, occurrence=1):
    """rotate_after applied to every item, in order."""
    return [rotate_after(t, mark, occurrence) for t in items]
