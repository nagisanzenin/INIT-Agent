"""The workbook: cells, dependency graph, cycle detection, incremental recalculation (SPEC.md section 8)."""
import heapq

from .evaluator import evaluate
from .parser import delete_formula, insert_formula, parse, references, shift_formula
from .refs import cell_name
from .tokenizer import ParseError
from .values import BLANK, CYCLE, PARSE, parse_content


class Cell:
    __slots__ = ("pos", "text", "tree", "refs", "rects", "value")

    def __init__(self, pos):
        self.pos = pos
        self.text = ""
        self.tree = None
        self.refs = []
        self.rects = []
        self.value = BLANK

    @property
    def is_formula(self):
        return self.text.startswith("=")

    def load(self, text):
        """Takes new content text; a formula's value is left for the next recalculation."""
        self.text = text
        self.tree = None
        self.refs, self.rects = [], []
        if not self.is_formula:
            self.value = parse_content(text)
            return
        try:
            self.tree = parse(text[1:])
        except ParseError:
            return
        self.refs, self.rects = references(self.tree)


class Workbook:
    def __init__(self):
        self.cells = {}
        self.dependents = {}  # position -> formula positions with a single-cell reference to it
        self.watchers = {}  # formula position -> the range rectangles its formula reads
        self.reverse = {}  # position -> cached result of dependents_of
        self.last_recalc = []

    # ------------------------------------------------------------------ content

    def value(self, pos):
        cell = self.cells.get(pos)
        return BLANK if cell is None else cell.value

    def is_formula(self, pos):
        cell = self.cells.get(pos)
        return cell is not None and cell.is_formula

    def used_size(self):
        """(rows, cols) of the used range: the largest row and column holding content."""
        if not self.cells:
            return (0, 0)
        return (max(p[0] for p in self.cells), max(p[1] for p in self.cells))

    def load_rows(self, rows):
        """Initial content from CSV records, then a full calculation."""
        for r, fields in enumerate(rows, 1):
            for c, text in enumerate(fields, 1):
                if text != "":
                    self._assign((r, c), text)
        self._recalc({p for p in self.cells if self.is_formula(p)})

    def set_cells(self, assignments):
        """One edit: [(pos, text), ...] applied in order, then the affected cells recalculated."""
        for pos, text in assignments:
            self._assign(pos, text)
        self._recalc(self._affected([pos for pos, _ in assignments]))

    def copy(self, src, rect):
        """The content of src copied to every cell of rect, references moved (one edit)."""
        cell = self.cells.get(src)
        text = "" if cell is None else cell.text
        out = []
        for pos in rect.cells():
            if text.startswith("="):
                out.append((pos, "=" + shift_formula(text[1:], pos[0] - src[0], pos[1] - src[1])))
            else:
                out.append((pos, text))
        self.set_cells(out)

    def restructure(self, axis, at, delete):
        """Inserts a row (axis 0) or column (axis 1) before number `at`, or deletes number `at`; cells move and
        every formula is rewritten (SPEC.md 9.5), then all formula cells are recalculated."""
        moved = {}
        for pos, cell in self.cells.items():
            i = pos[axis]
            if delete and i == at:
                continue
            if i >= at:
                i += -1 if delete else 1
            text = cell.text
            if cell.is_formula:
                rewrite = delete_formula if delete else insert_formula
                text = "=" + rewrite(text[1:], axis, at)
            moved[(pos[0], i) if axis else (i, pos[1])] = text
        self.cells, self.dependents, self.watchers, self.reverse = {}, {}, {}, {}
        for pos in sorted(moved):
            self._assign(pos, moved[pos])
        self._recalc({p for p in self.cells if self.is_formula(p)})

    def _assign(self, pos, text):
        cell = self.cells.get(pos)
        if cell is None:
            cell = self.cells[pos] = Cell(pos)
        cell.load(text)
        self._unlink(cell)
        self._link(cell)
        if text == "":
            del self.cells[pos]

    def _link(self, cell):
        for p in cell.refs:
            self.dependents.setdefault(p, set()).add(cell.pos)
        if cell.rects:
            self.watchers[cell.pos] = list(cell.rects)
            self.reverse.clear()

    def _unlink(self, cell):
        for p in cell.refs:
            deps = self.dependents.get(p)
            if deps is not None:
                deps.discard(cell.pos)
                if not deps:
                    del self.dependents[p]
        self.watchers.pop(cell.pos, None)

    # ------------------------------------------------------------------ graph

    def dependents_of(self, pos):
        """Formula positions that read pos directly, through a single-cell reference or a range."""
        out = self.reverse.get(pos)
        if out is None:
            out = set(self.dependents.get(pos, ()))
            for f, rects in self.watchers.items():
                if any(r.contains(pos) for r in rects):
                    out.add(f)
            self.reverse[pos] = out
        return out

    def _precedents_in(self, cell, group):
        """The positions of `group` that the formula of `cell` reads."""
        out = [p for p in cell.refs if p in group]
        for r in cell.rects:
            out.extend(p for p in group if r.contains(p))
        return out

    def _affected(self, edited):
        """The edited formula cells and every formula cell that depends on an edited cell."""
        out = {p for p in edited if self.is_formula(p)}
        seen = set(edited)
        stack = list(edited)
        while stack:
            for d in self.dependents_of(stack.pop()):
                if d not in seen:
                    seen.add(d)
                    stack.append(d)
                    if self.is_formula(d):
                        out.add(d)
        return out

    def _cycle_members(self, group):
        """The positions of `group` that lie on a dependency cycle (Tarjan's strongly connected components)."""
        succ = {v: sorted(d for d in self.dependents_of(v) if d in group) for v in group}
        index, low, stack, on_stack, members = {}, {}, [], set(), set()

        def visit(v):
            index[v] = low[v] = len(index)
            stack.append(v)
            on_stack.add(v)
            for w in succ[v]:
                if w not in index:
                    visit(w)
                    low[v] = min(low[v], low[w])
                elif w in on_stack:
                    low[v] = min(low[v], index[w])
            if low[v] == index[v]:
                comp = []
                while True:
                    w = stack.pop()
                    on_stack.discard(w)
                    comp.append(w)
                    if w == v:
                        break
                if len(comp) > 1 or v in succ[v]:
                    members.update(comp)

        for root in sorted(group):
            if root not in index:
                visit(root)
        return members

    def _recalc(self, todo):
        """Recalculates the formula cells in todo: cycle members get #CYCLE!, the others are evaluated once each
        in dependency order, the first ready cell in row-major order first."""
        cyclic = self._cycle_members(todo)
        for p in cyclic:
            self.cells[p].value = CYCLE
        live = todo - cyclic
        waiting = {p: len(set(self._precedents_in(self.cells[p], live))) for p in live}
        ready = [p for p in live if waiting[p] == 0]
        heapq.heapify(ready)
        order = []
        while ready:
            p = heapq.heappop(ready)
            cell = self.cells[p]
            cell.value = PARSE if cell.tree is None else evaluate(cell.tree, self.value)
            order.append(p)
            for d in self.dependents_of(p):
                if d in live:
                    waiting[d] -= 1
                    if waiting[d] == 0:
                        heapq.heappush(ready, d)
        self.last_recalc = order

    def recalc_names(self):
        return [cell_name(p) for p in self.last_recalc]
