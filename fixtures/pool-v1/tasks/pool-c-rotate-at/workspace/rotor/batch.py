from .core import rotate_after


def rotate_all(items, letter):
    """rotate_after applied to every item, in order."""
    return [rotate_after(t, letter) for t in items]
