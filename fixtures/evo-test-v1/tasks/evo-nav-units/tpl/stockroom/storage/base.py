"""The Store interface and the atomic file write every store uses."""
import os

from ..errors import StorageError
from ..models.inventory import Inventory


class Store:
    """A place an Inventory is loaded from and saved to."""

    kind = "abstract"

    def __init__(self, path):
        self.path = path

    def exists(self) -> bool:
        raise NotImplementedError

    def load(self) -> Inventory:
        raise NotImplementedError

    def save(self, inv: Inventory) -> None:
        raise NotImplementedError

    def init(self) -> None:
        """Creates an empty store; refuses to overwrite an existing one."""
        if self.exists():
            raise StorageError("store already exists: %s" % self.path)
        self.save(Inventory())

    def describe(self) -> str:
        return "%s store at %s" % (self.kind, self.path)


def atomic_write(path, text) -> None:
    """Writes `text` to a temporary file next to `path`, then renames it over `path`."""
    tmp = path + ".tmp"
    with open(tmp, "w", encoding="utf-8", newline="") as f:
        f.write(text)
    os.replace(tmp, path)


def read_text(path, what) -> str:
    try:
        with open(path, encoding="utf-8", newline="") as f:
            return f.read()
    except OSError as e:
        raise StorageError("cannot read %s: %s" % (what, e.strerror or e))
