"""Dev fixture: the unmutated exercise's behaviour adapted to the new interface: Conway's B3/S23 rule on a
bounded board with the 8-cell neighbourhood. gen/run fills in only the seeded cell characters (live, dead).
The validator checks that this scores below 1000 on every instance."""
P = None  # @@PARAMS@@


def evolve(grid, steps):
    if steps < 0:
        raise ValueError("negative steps")
    if any(len(row) != len(grid[0]) for row in grid):
        raise ValueError("ragged grid")
    if any(ch not in (P["live"], P["dead"]) for row in grid for ch in row):
        raise ValueError("bad cell")
    board = [[1 if ch == P["live"] else 0 for ch in row] for row in grid]
    for _ in range(steps):
        h = len(board)
        w = len(board[0]) if h else 0
        new = [[0] * w for _ in range(h)]
        for r in range(h):
            for c in range(w):
                n = sum(board[i][j]
                        for i in range(max(0, r - 1), min(h, r + 2))
                        for j in range(max(0, c - 1), min(w, c + 2))) - board[r][c]
                new[r][c] = 1 if n == 3 or (board[r][c] and n == 2) else 0
        board = new
    return ["".join(P["live"] if x else P["dead"] for x in row) for row in board]
