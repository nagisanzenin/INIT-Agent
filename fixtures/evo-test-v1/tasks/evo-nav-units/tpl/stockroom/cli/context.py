"""What every command function receives besides its arguments."""


class Ctx:
    def __init__(self, store, config):
        self.store = store
        self.config = config

    def load(self):
        return self.store.load()

    def save(self, inv):
        self.store.save(inv)
