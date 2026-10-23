"""Kite resolver: binds every name in the syntax tree to a local slot, a free variable or a global.

It walks the tree once, in source order, keeping a stack of block scopes per function. A name refers to the
innermost declaration that precedes it; a declaration of the enclosing function that a nested function refers to
is marked `captured` and becomes one of the nested function's free variables. Names that resolve to nothing are
globals. The resolver also reports the static errors of SPEC.md (redeclaration, misplaced break/continue/return).
"""
from . import nodes as N
from .errors import KiteSyntaxError


class Var:
    """One declared local: its slot in the frame and whether a nested function captures it."""
    __slots__ = ("name", "slot", "captured", "line")

    def __init__(self, name, slot, line):
        self.name = name
        self.slot = slot
        self.captured = False
        self.line = line

    def __repr__(self):
        return "Var(%s, slot %d%s)" % (self.name, self.slot, ", captured" if self.captured else "")


class FunctionInfo:
    """What the compiler needs to know about one function."""
    __slots__ = ("name", "params", "nslots", "free", "locals")

    def __init__(self, name):
        self.name = name
        self.params = []   # Var per parameter, slots 0..n-1
        self.nslots = 0
        self.free = []     # (is_local, index) per free variable: a slot of the enclosing frame or one of its free vars
        self.locals = []   # every Var declared in this function, parameters included

    def captured_params(self):
        return [v.slot for v in self.params if v.captured]

    def captured_locals(self):
        return [v.slot for v in self.locals[len(self.params):] if v.captured]


class FunctionScope:
    def __init__(self, parent, info, is_main):
        self.parent = parent
        self.info = info
        self.is_main = is_main
        self.blocks = [{}]
        self.loop_depth = 0
        self.free_index = {}

    def lookup(self, name):
        for block in reversed(self.blocks):
            var = block.get(name)
            if var is not None:
                return var
        return None

    def declare(self, name, line):
        block = self.blocks[-1]
        if name in block:
            raise KiteSyntaxError("'%s' is already declared in this block" % name, line)
        var = Var(name, self.info.nslots, line)
        self.info.nslots += 1
        self.info.locals.append(var)
        block[name] = var
        return var

    def add_free(self, key, is_local, index):
        i = self.free_index.get(key)
        if i is None:
            i = len(self.info.free)
            self.info.free.append((is_local, index))
            self.free_index[key] = i
        return i

    def at_global_level(self):
        return self.is_main and len(self.blocks) == 1


class Resolver:
    def resolve_program(self, stmts):
        info = FunctionInfo("main")
        fs = FunctionScope(None, info, True)
        for stmt in stmts:
            self.stmt(fs, stmt)
        return info

    # ------------------------------------------------------------------------------------------------ names

    def resolve_name(self, fs, name):
        var = fs.lookup(name)
        if var is not None:
            return ("cell" if var.captured else "local", var)
        index = self.resolve_free(fs, name)
        if index is not None:
            return ("free", index)
        return ("global", name)

    def resolve_free(self, fs, name):
        parent = fs.parent
        if parent is None:
            return None
        var = parent.lookup(name)
        if var is not None:
            var.captured = True
            return fs.add_free(var.slot, True, var.slot)
        index = self.resolve_free(parent, name)
        if index is not None:
            return fs.add_free(index, False, index)
        return None

    # ------------------------------------------------------------------------------------------- statements

    def stmt(self, fs, node):
        t = type(node)
        if t is N.ExprStmt:
            self.expr(fs, node.expr)
        elif t is N.Let:
            if node.init is not None:
                self.expr(fs, node.init)
            node.var = None if fs.at_global_level() else fs.declare(node.name, node.line)
        elif t is N.FnDecl:
            if fs.at_global_level():
                node.var = None
            else:
                node.var = fs.declare(node.name, node.line)
            self.function(fs, node.function)
        elif t is N.Block:
            self.block(fs, node.stmts)
        elif t is N.If:
            self.expr(fs, node.cond)
            self.block(fs, node.then.stmts)
            if node.orelse is not None:
                self.stmt(fs, node.orelse)
        elif t is N.While:
            self.expr(fs, node.cond)
            fs.loop_depth += 1
            self.block(fs, node.body.stmts)
            fs.loop_depth -= 1
        elif t is N.For:
            fs.blocks.append({})
            if node.init is not None:
                self.stmt(fs, node.init)
            if node.cond is not None:
                self.expr(fs, node.cond)
            if node.step is not None:
                self.expr(fs, node.step)
            fs.loop_depth += 1
            self.block(fs, node.body.stmts)
            fs.loop_depth -= 1
            fs.blocks.pop()
        elif t is N.Return:
            if fs.is_main:
                raise KiteSyntaxError("'return' outside a function", node.line)
            if node.value is not None:
                self.expr(fs, node.value)
        elif t is N.Break or t is N.Continue:
            if fs.loop_depth == 0:
                raise KiteSyntaxError("'%s' outside a loop" % ("break" if t is N.Break else "continue"), node.line)
        else:
            raise TypeError("unknown statement %r" % node)

    def block(self, fs, stmts):
        fs.blocks.append({})
        for s in stmts:
            self.stmt(fs, s)
        fs.blocks.pop()

    def function(self, fs, node):
        info = FunctionInfo(node.name)
        inner = FunctionScope(fs, info, False)
        for pname, pline in node.params:
            info.params.append(inner.declare(pname, pline))
        for s in node.body:
            self.stmt(inner, s)
        node.info = info

    # ------------------------------------------------------------------------------------------ expressions

    def expr(self, fs, node):
        t = type(node)
        if t is N.Literal:
            return
        if t is N.Name:
            node.ref = self.resolve_name(fs, node.name)
        elif t is N.ListLit:
            for item in node.items:
                self.expr(fs, item)
        elif t is N.MapLit:
            for key, value in node.pairs:
                self.expr(fs, key)
                self.expr(fs, value)
        elif t is N.Index:
            self.expr(fs, node.obj)
            self.expr(fs, node.index)
        elif t is N.Call:
            self.expr(fs, node.callee)
            for a in node.args:
                self.expr(fs, a)
        elif t is N.Unary:
            self.expr(fs, node.operand)
        elif t is N.Binary or t is N.Logical:
            self.expr(fs, node.left)
            self.expr(fs, node.right)
        elif t is N.Assign:
            self.expr(fs, node.target)
            self.expr(fs, node.value)
        elif t is N.Function:
            self.function(fs, node)
        else:
            raise TypeError("unknown expression %r" % node)


def resolve(stmts):
    return Resolver().resolve_program(stmts)
