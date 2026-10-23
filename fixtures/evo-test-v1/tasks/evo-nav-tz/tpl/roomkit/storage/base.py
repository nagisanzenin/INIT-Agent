"""The store interface. A store loads a whole Calendar and saves a whole Calendar."""
import os


class Store:
    kind = "?"

    def __init__(self, path):
        self.path = str(path)

    def load(self):
        """The Calendar in the store (empty when the store does not exist yet)."""
        raise NotImplementedError

    def save(self, cal):
        """Write the whole calendar at the newest schema version."""
        raise NotImplementedError

    def exists(self):
        return os.path.exists(self.path)

    def update(self, fn):
        """Load, apply fn(cal), save, return what fn returned. Nothing is saved when fn raises."""
        cal = self.load()
        result = fn(cal)
        self.save(cal)
        return result

    def name(self):
        return os.path.basename(os.path.normpath(self.path)) or self.path

    def describe(self):
        return "%s store at %s" % (self.kind, self.path)
