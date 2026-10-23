"""Dev fixture: the unmutated exercise's behaviour adapted to the new interface: plain string comparison,
a value equal to or smaller than the node goes left, path letters 'L' / 'R'.
The validator checks that this scores below 1000 on every instance."""


class TreeNode:
    def __init__(self, data, left=None, right=None):
        self.data = data
        self.left = left
        self.right = right


class BinarySearchTree:
    def __init__(self, tree_data):
        self.root = None
        for item in tree_data:
            if self.root is None:
                self.root = TreeNode(item)
                continue
            node = self.root
            while True:
                if item <= node.data:
                    if node.left is None:
                        node.left = TreeNode(item)
                        break
                    node = node.left
                else:
                    if node.right is None:
                        node.right = TreeNode(item)
                        break
                    node = node.right

    def sorted_data(self):
        def walk(node):
            return [] if node is None else walk(node.left) + [node.data] + walk(node.right)
        return walk(self.root)

    def height(self):
        def depth(node):
            return 0 if node is None else 1 + max(depth(node.left), depth(node.right))
        return depth(self.root)

    def levels(self):
        result, frontier = [], [self.root] if self.root else []
        while frontier:
            result.append([n.data for n in frontier])
            frontier = [c for n in frontier for c in (n.left, n.right) if c]
        return result

    def path(self, value):
        node, trail = self.root, ""
        while node is not None:
            if value == node.data:
                return trail
            if value < node.data:
                node, trail = node.left, trail + "L"
            else:
                node, trail = node.right, trail + "R"
        raise ValueError("not found")
