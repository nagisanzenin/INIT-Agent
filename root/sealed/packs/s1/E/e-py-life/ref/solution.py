"""Reference solution for e-py-life. gen/run replaces the P line with the instance's parameters:
live / dead (the two cell characters), birth / survive (lists of neighbour counts), wrap (the board is a
torus) and hood ("moore": the 8 surrounding cells, "vn": the 4 orthogonal cells)."""
P = None  # @@PARAMS@@

_MOORE = ((-1, -1), (-1, 0), (-1, 1), (0, -1), (0, 1), (1, -1), (1, 0), (1, 1))
_VN = ((-1, 0), (0, -1), (0, 1), (1, 0))


def _validate(grid, steps):
    if steps < 0:
        raise ValueError("steps must not be negative")
    width = len(grid[0]) if grid else 0
    for row in grid:
        if len(row) != width:
            raise ValueError("rows differ in length")
        for ch in row:
            if ch != P["live"] and ch != P["dead"]:
                raise ValueError("unknown cell character %r" % ch)


def _generation(cells):
    h, w = len(cells), len(cells[0])
    offsets = _MOORE if P["hood"] == "moore" else _VN
    nxt = []
    for r in range(h):
        row = []
        for c in range(w):
            n = 0
            for dr, dc in offsets:
                rr, cc = r + dr, c + dc
                if P["wrap"]:
                    rr, cc = rr % h, cc % w
                elif not (0 <= rr < h and 0 <= cc < w):
                    continue
                if cells[rr][cc]:
                    n += 1
            row.append(n in P["survive"] if cells[r][c] else n in P["birth"])
        nxt.append(row)
    return nxt


def evolve(grid, steps):
    _validate(grid, steps)
    cells = [[ch == P["live"] for ch in row] for row in grid]
    if cells and cells[0]:
        for _ in range(steps):
            cells = _generation(cells)
    return ["".join(P["live"] if alive else P["dead"] for alive in row) for row in cells]
