"""Reference solution for e-py-bst. gen/run replaces the P line with the instance's parameters:
key ("numeric" | "numeric_desc" | "text_desc" | "digit_sum": which value goes left), dup ("left" | "right" |
"ignore": where an equal value goes) and steps (the two path letters: [left, right])."""
P = None  # @@PARAMS@@


def _order(a, b):
    """-1 when a belongs left of b, 1 when right, 0 when equal (equal means the same string)."""
    if a == b:
        return 0
    key = P["key"]
    if key == "numeric":
        return -1 if int(a) < int(b) else 1
    if key == "numeric_desc":
        return -1 if int(a) > int(b) else 1
    if key == "text_desc":
        return -1 if a > b else 1
    ka = (sum(int(ch) for ch in a), int(a))
    kb = (sum(int(ch) for ch in b), int(b))
    return -1 if ka < kb else 1


class _Node:
    def __init__(self, value):
        self.value = value
        self.left = None
        self.right = None


class BinarySearchTree:
    def __init__(self, values):
        self._root = None
        for v in values:
            self._insert(v)

    def _insert(self, v):
        if self._root is None:
            self._root = _Node(v)
            return
        node = self._root
        while True:
            c = _order(v, node.value)
            if c == 0:
                if P["dup"] == "ignore":
                    return
                c = -1 if P["dup"] == "left" else 1
            if c < 0:
                if node.left is None:
                    node.left = _Node(v)
                    return
                node = node.left
            else:
                if node.right is None:
                    node.right = _Node(v)
                    return
                node = node.right

    def sorted_data(self):
        out = []

        def walk(n):
            if n is not None:
                walk(n.left)
                out.append(n.value)
                walk(n.right)
        walk(self._root)
        return out

    def height(self):
        def h(n):
            return 0 if n is None else 1 + max(h(n.left), h(n.right))
        return h(self._root)

    def levels(self):
        out = []
        row = [self._root] if self._root is not None else []
        while row:
            out.append([n.value for n in row])
            row = [c for n in row for c in (n.left, n.right) if c is not None]
        return out

    def path(self, value):
        steps = []
        node = self._root
        while node is not None:
            c = _order(value, node.value)
            if c == 0:
                return "".join(steps)
            if c < 0:
                steps.append(P["steps"][0])
                node = node.left
            else:
                steps.append(P["steps"][1])
                node = node.right
        raise ValueError("value not in tree: %r" % (value,))
