"""Syntax tree of a Kite program. The parser builds it, the resolver annotates it, the compiler reads it.

Every node carries the line of the token that identifies it (an operator, a keyword, a name, the `(` of a call,
the `[` of an indexing).
"""


class Node:
    __slots__ = ("line",)


# ------------------------------------------------------------------------------------------------ expressions

class Literal(Node):
    __slots__ = ("value",)

    def __init__(self, value, line):
        self.value = value
        self.line = line


class Name(Node):
    __slots__ = ("name", "ref")

    def __init__(self, name, line):
        self.name = name
        self.line = line
        self.ref = None  # set by the resolver: ("local", Var) | ("cell", Var) | ("free", index) | ("global", name)


class ListLit(Node):
    __slots__ = ("items",)

    def __init__(self, items, line):
        self.items = items
        self.line = line


class MapLit(Node):
    __slots__ = ("pairs",)

    def __init__(self, pairs, line):
        self.pairs = pairs  # list of (key expression, value expression)
        self.line = line


class Index(Node):
    __slots__ = ("obj", "index")

    def __init__(self, obj, index, line):
        self.obj = obj
        self.index = index
        self.line = line


class Call(Node):
    __slots__ = ("callee", "args")

    def __init__(self, callee, args, line):
        self.callee = callee
        self.args = args
        self.line = line


class Unary(Node):
    __slots__ = ("op", "operand")

    def __init__(self, op, operand, line):
        self.op = op
        self.operand = operand
        self.line = line


class Binary(Node):
    __slots__ = ("op", "left", "right")

    def __init__(self, op, left, right, line):
        self.op = op
        self.left = left
        self.right = right
        self.line = line


class Logical(Node):
    __slots__ = ("op", "left", "right")

    def __init__(self, op, left, right, line):
        self.op = op
        self.left = left
        self.right = right
        self.line = line


class Assign(Node):
    """target op= value; op is "=" or one of "+=", "-=", "*=", "/=", "%="."""
    __slots__ = ("target", "op", "value")

    def __init__(self, target, op, value, line):
        self.target = target
        self.op = op
        self.value = value
        self.line = line


class Function(Node):
    """A function literal or the function of a `fn name(...) {...}` declaration."""
    __slots__ = ("name", "params", "body", "info")

    def __init__(self, name, params, body, line):
        self.name = name
        self.params = params  # list of (name, line)
        self.body = body      # list of statements
        self.line = line
        self.info = None      # set by the resolver: scope.FunctionInfo


# ------------------------------------------------------------------------------------------------- statements

class ExprStmt(Node):
    __slots__ = ("expr",)

    def __init__(self, expr, line):
        self.expr = expr
        self.line = line


class Let(Node):
    __slots__ = ("name", "init", "var")

    def __init__(self, name, init, line):
        self.name = name
        self.init = init
        self.line = line
        self.var = None  # set by the resolver: scope.Var, or None for a global


class FnDecl(Node):
    __slots__ = ("name", "function", "var")

    def __init__(self, name, function, line):
        self.name = name
        self.function = function
        self.line = line
        self.var = None  # set by the resolver: scope.Var, or None for a global


class If(Node):
    __slots__ = ("cond", "then", "orelse")

    def __init__(self, cond, then, orelse, line):
        self.cond = cond
        self.then = then
        self.orelse = orelse  # None, a Block or an If
        self.line = line


class While(Node):
    __slots__ = ("cond", "body")

    def __init__(self, cond, body, line):
        self.cond = cond
        self.body = body
        self.line = line


class For(Node):
    __slots__ = ("init", "cond", "step", "body")

    def __init__(self, init, cond, step, body, line):
        self.init = init  # None, a Let or an ExprStmt
        self.cond = cond  # None or an expression
        self.step = step  # None or an expression
        self.body = body
        self.line = line


class Return(Node):
    __slots__ = ("value",)

    def __init__(self, value, line):
        self.value = value
        self.line = line


class Break(Node):
    __slots__ = ()

    def __init__(self, line):
        self.line = line


class Continue(Node):
    __slots__ = ()

    def __init__(self, line):
        self.line = line


class Block(Node):
    __slots__ = ("stmts",)

    def __init__(self, stmts, line):
        self.stmts = stmts
        self.line = line
