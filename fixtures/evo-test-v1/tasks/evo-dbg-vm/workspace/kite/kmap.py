"""Kite maps: insertion-ordered hash maps with Kite's key semantics (1 and true are different keys).

The layout follows CPython's compact dict: `entries` holds [key, value] pairs in insertion order (None where an
entry was removed) and `slots` is an open-addressing table (linear probing) whose cells hold an index into
`entries`, or EMPTY, or DELETED for a cell whose entry was removed. Hashes are computed here, so a map behaves
the same whatever Python's hash randomization does.
"""
EMPTY = -1
DELETED = -2
MIN_SIZE = 8


def key_hash(key):
    if type(key) is str:
        h = 2166136261
        for ch in key:
            h = ((h ^ ord(ch)) * 16777619) & 0xFFFFFFFF
        return h
    if key is True:
        return 1
    if key is False:
        return 0
    return key & 0xFFFFFFFF


class KMap:
    __slots__ = ("entries", "slots", "count", "filled")

    def __init__(self):
        self.entries = []
        self.slots = [EMPTY] * MIN_SIZE
        self.count = 0   # live entries
        self.filled = 0  # slots that are not EMPTY (live or DELETED)

    def find(self, key):
        """(slot of key or -1, slot where key would be inserted)."""
        mask = len(self.slots) - 1
        i = key_hash(key) & mask
        while True:
            ix = self.slots[i]
            if ix < 0:
                return -1, i
            k = self.entries[ix][0]
            if type(k) is type(key) and k == key:
                return i, i
            i = (i + 1) & mask

    def get(self, key, default=None):
        s, _ = self.find(key)
        if s < 0:
            return default
        return self.entries[self.slots[s]][1]

    def has(self, key):
        return self.find(key)[0] >= 0

    def set(self, key, value):
        s, free = self.find(key)
        if s >= 0:
            self.entries[self.slots[s]][1] = value
            return
        if self.slots[free] == EMPTY:
            self.filled += 1
        self.slots[free] = len(self.entries)
        self.entries.append([key, value])
        self.count += 1
        if self.filled * 3 >= len(self.slots) * 2:
            self.resize()

    def remove(self, key):
        """Removes key and returns (True, value), or (False, None) if it is absent."""
        s, _ = self.find(key)
        if s < 0:
            return False, None
        ix = self.slots[s]
        value = self.entries[ix][1]
        self.entries[ix] = None
        self.slots[s] = DELETED
        self.count -= 1
        return True, value

    def resize(self):
        size = MIN_SIZE
        while size <= self.count * 2:
            size *= 2
        mask = size - 1
        slots = [EMPTY] * size
        for ix, entry in enumerate(self.entries):
            if entry is None:
                continue
            i = key_hash(entry[0]) & mask
            while slots[i] != EMPTY:
                i = (i + 1) & mask
            slots[i] = ix
        self.entries = [e for e in self.entries if e is not None]
        self.slots = slots
        self.filled = self.count

    def keys(self):
        return [e[0] for e in self.entries if e is not None]

    def items(self):
        return [(e[0], e[1]) for e in self.entries if e is not None]
