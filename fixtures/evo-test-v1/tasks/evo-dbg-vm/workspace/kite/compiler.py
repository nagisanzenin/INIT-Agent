"""Kite compiler: turns the resolved syntax tree into bytecode, one Proto per function.

An instruction is an (opcode, argument) pair; `lines` holds the source line of every instruction, which the VM
reports when that instruction fails. Jump arguments are absolute instruction indexes.

Opcodes (stack effect):
  CONST k          push consts[k]                 NIL / TRUE / FALSE   push the literal
  POP              drop the top                   DUP2                 a b -> a b a b
  LOAD_LOCAL s     push slots[s]                  SET_LOCAL s          slots[s] = pop()
  STORE_LOCAL s    slots[s] = top (kept)          MAKE_CELL s          slots[s] = new cell holding pop()
  LOAD_CELL s      push slots[s].value            STORE_CELL s         slots[s].value = top (kept)
  LOAD_FREE i      push cells[i].value            STORE_FREE i         cells[i].value = top (kept)
  LOAD_GLOBAL n    push globals[n]                STORE_GLOBAL n       globals[n] = top (kept; must exist)
  DEFINE_GLOBAL n  globals[n] = pop()
  BINARY op        a b -> a op b                  NEG / NOT            a -> -a / not a
  BUILD_LIST n     n items -> list                INDEX_GET            obj i -> obj[i]
  BUILD_MAP n      k1 v1 .. kn vn -> map
  INDEX_SET        obj i v -> v (obj[i] = v)      CALL n               f a1..an -> result
  CLOSURE k        push a closure of consts[k]    RETURN               return pop() to the caller
  JUMP t           ip = t                         JUMP_IF_FALSE t      if not truthy(pop()): ip = t
  JUMP_IF_FALSE_OR_POP t   if not truthy(top): ip = t (top kept) else pop()
  JUMP_IF_TRUE_OR_POP t    if truthy(top): ip = t (top kept) else pop()
"""
from . import nodes as N


class Proto:
    """A compiled function."""
    __slots__ = ("name", "nparams", "nslots", "free", "code", "lines", "consts", "global_cache")

    def __init__(self, name, nparams, nslots, free):
        self.name = name
        self.nparams = nparams
        self.nslots = nslots
        self.free = list(free)
        self.code = []
        self.lines = []
        self.consts = []
        self.global_cache = {}  # filled by the VM: instruction index -> (globals version, value)

    def __repr__(self):
        return "<proto %s>" % (self.name or "fn")


class Loop:
    __slots__ = ("continue_target", "breaks", "continues")

    def __init__(self, continue_target):
        self.continue_target = continue_target  # None: continues are forward jumps, patched later
        self.breaks = []
        self.continues = []


class FunctionCompiler:
    def __init__(self, info, name, nparams):
        self.info = info
        self.proto = Proto(name, nparams, info.nslots, info.free)
        self.code = self.proto.code
        self.lines = self.proto.lines
        self.const_index = {}
        self.loops = []

    # --------------------------------------------------------------------------------------------- emitting

    def emit(self, op, arg, line):
        self.code.append((op, arg))
        self.lines.append(line)
        return len(self.code) - 1

    def here(self):
        return len(self.code)

    def patch(self, at, target=None):
        op, _ = self.code[at]
        self.code[at] = (op, self.here() if target is None else target)

    def const(self, value):
        if isinstance(value, Proto):
            self.proto.consts.append(value)
            return len(self.proto.consts) - 1
        key = (type(value), value)
        k = self.const_index.get(key)
        if k is None:
            k = len(self.proto.consts)
            self.proto.consts.append(value)
            self.const_index[key] = k
        return k

    # ------------------------------------------------------------------------------------------------ bodies

    def compile_body(self, stmts, line):
        for slot in self.info.captured_params():
            self.emit("LOAD_LOCAL", slot, line)
            self.emit("MAKE_CELL", slot, line)
        for slot in self.info.captured_locals():
            self.emit("NIL", None, line)
            self.emit("MAKE_CELL", slot, line)
        for s in stmts:
            self.stmt(s)
        self.emit("NIL", None, line)
        self.emit("RETURN", None, line)
        return self.proto

    def function(self, node):
        fc = FunctionCompiler(node.info, node.name, len(node.params))
        proto = fc.compile_body(node.body, node.line)
        self.emit("CLOSURE", self.const(proto), node.line)

    # ----------------------------------------------------------------------------------------- variables

    def load(self, ref, line):
        kind, what = ref
        if kind == "local":
            self.emit("LOAD_LOCAL", what.slot, line)
        elif kind == "cell":
            self.emit("LOAD_CELL", what.slot, line)
        elif kind == "free":
            self.emit("LOAD_FREE", what, line)
        else:
            self.emit("LOAD_GLOBAL", what, line)

    def store(self, ref, line):
        kind, what = ref
        if kind == "local":
            self.emit("STORE_LOCAL", what.slot, line)
        elif kind == "cell":
            self.emit("STORE_CELL", what.slot, line)
        elif kind == "free":
            self.emit("STORE_FREE", what, line)
        else:
            self.emit("STORE_GLOBAL", what, line)

    def define(self, var, name, line):
        """Binds the value on top of the stack to a newly declared variable."""
        if var is None:
            self.emit("DEFINE_GLOBAL", name, line)
        elif var.captured:
            self.emit("STORE_CELL", var.slot, line)
            self.emit("POP", None, line)
        else:
            self.emit("SET_LOCAL", var.slot, line)

    # ------------------------------------------------------------------------------------------- statements

    def stmts(self, stmts):
        for s in stmts:
            self.stmt(s)

    def stmt(self, node):
        t = type(node)
        if t is N.ExprStmt:
            self.expr(node.expr)
            self.emit("POP", None, node.line)
        elif t is N.Let:
            if node.init is None:
                self.emit("NIL", None, node.line)
            else:
                self.expr(node.init)
            self.define(node.var, node.name, node.line)
        elif t is N.FnDecl:
            self.function(node.function)
            self.define(node.var, node.name, node.line)
        elif t is N.Block:
            self.stmts(node.stmts)
        elif t is N.If:
            self.if_stmt(node)
        elif t is N.While:
            self.while_stmt(node)
        elif t is N.For:
            self.for_stmt(node)
        elif t is N.Return:
            if node.value is None:
                self.emit("NIL", None, node.line)
            else:
                self.expr(node.value)
            self.emit("RETURN", None, node.line)
        elif t is N.Break:
            self.loops[-1].breaks.append(self.emit("JUMP", None, node.line))
        elif t is N.Continue:
            loop = self.loops[-1]
            if loop.continue_target is not None:
                self.emit("JUMP", loop.continue_target, node.line)
            else:
                loop.continues.append(self.emit("JUMP", None, node.line))
        else:
            raise TypeError("unknown statement %r" % node)

    def if_stmt(self, node):
        self.expr(node.cond)
        to_else = self.emit("JUMP_IF_FALSE", None, node.line)
        self.stmts(node.then.stmts)
        if node.orelse is None:
            self.patch(to_else)
            return
        to_end = self.emit("JUMP", None, node.line)
        self.patch(to_else)
        self.stmt(node.orelse)
        self.patch(to_end)

    def while_stmt(self, node):
        start = self.here()
        self.expr(node.cond)
        to_exit = self.emit("JUMP_IF_FALSE", None, node.line)
        loop = Loop(start)
        self.loops.append(loop)
        self.stmts(node.body.stmts)
        self.emit("JUMP", start, node.line)
        self.loops.pop()
        self.patch(to_exit)
        for j in loop.breaks:
            self.patch(j)

    def for_stmt(self, node):
        if node.init is not None:
            self.stmt(node.init)
        start = self.here()
        to_exit = None
        if node.cond is not None:
            self.expr(node.cond)
            to_exit = self.emit("JUMP_IF_FALSE", None, node.line)
        loop = Loop(start)
        self.loops.append(loop)
        self.stmts(node.body.stmts)
        for j in loop.continues:
            self.patch(j)
        if node.step is not None:
            self.expr(node.step)
            self.emit("POP", None, node.line)
        self.emit("JUMP", start, node.line)
        self.loops.pop()
        if to_exit is not None:
            self.patch(to_exit)
        for j in loop.breaks:
            self.patch(j)

    # ------------------------------------------------------------------------------------------ expressions

    def expr(self, node):
        t = type(node)
        if t is N.Literal:
            v = node.value
            if v is None:
                self.emit("NIL", None, node.line)
            elif v is True:
                self.emit("TRUE", None, node.line)
            elif v is False:
                self.emit("FALSE", None, node.line)
            else:
                self.emit("CONST", self.const(v), node.line)
        elif t is N.Name:
            self.load(node.ref, node.line)
        elif t is N.ListLit:
            for item in node.items:
                self.expr(item)
            self.emit("BUILD_LIST", len(node.items), node.line)
        elif t is N.MapLit:
            for key, value in node.pairs:
                self.expr(key)
                self.expr(value)
            self.emit("BUILD_MAP", len(node.pairs), node.line)
        elif t is N.Index:
            self.expr(node.obj)
            self.expr(node.index)
            self.emit("INDEX_GET", None, node.line)
        elif t is N.Call:
            self.expr(node.callee)
            for a in node.args:
                self.expr(a)
            self.emit("CALL", len(node.args), node.line)
        elif t is N.Unary:
            self.expr(node.operand)
            self.emit("NEG" if node.op == "-" else "NOT", None, node.line)
        elif t is N.Binary:
            self.expr(node.left)
            self.expr(node.right)
            self.emit("BINARY", node.op, node.line)
        elif t is N.Logical:
            self.expr(node.left)
            op = "JUMP_IF_FALSE_OR_POP" if node.op == "and" else "JUMP_IF_TRUE_OR_POP"
            to_end = self.emit(op, None, node.line)
            self.expr(node.right)
            self.patch(to_end)
        elif t is N.Assign:
            self.assign(node)
        elif t is N.Function:
            self.function(node)
        else:
            raise TypeError("unknown expression %r" % node)

    def assign(self, node):
        target = node.target
        binop = node.op[:-1]
        if type(target) is N.Name:
            if binop:
                self.load(target.ref, target.line)
                self.expr(node.value)
                self.emit("BINARY", binop, node.line)
            else:
                self.expr(node.value)
            self.store(target.ref, target.line)
            return
        self.expr(target.obj)
        self.expr(target.index)
        if binop:
            self.emit("DUP2", None, node.line)
            self.emit("INDEX_GET", None, target.line)
            self.expr(node.value)
            self.emit("BINARY", binop, node.line)
        else:
            self.expr(node.value)
        self.emit("INDEX_SET", None, node.line)


def compile_program(stmts, info):
    fc = FunctionCompiler(info, None, 0)
    return fc.compile_body(stmts, 1)
