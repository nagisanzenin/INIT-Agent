"""Kite runtime values: type names, display, equality, the operators and the builtin functions.

Kite values are Python values: int, str, bool (true/false), None (nil), list, plus KMap (kmap.py) and the Cell,
Closure and Builtin classes below. Python's bool is a subclass of int, so every type test here uses `type(v) is int`.
"""
from .errors import KiteRuntimeError
from .kmap import KMap

MISSING = object()


class Cell:
    """A captured variable, shared by the frame that declared it and every closure that refers to it."""
    __slots__ = ("value",)

    def __init__(self, value):
        self.value = value


class Closure:
    __slots__ = ("proto", "cells")

    def __init__(self, proto, cells):
        self.proto = proto
        self.cells = cells


class Builtin:
    __slots__ = ("name", "arity", "fn")

    def __init__(self, name, arity, fn):
        self.name = name
        self.arity = arity  # None: any number of arguments
        self.fn = fn

    def call(self, args):
        if self.arity is not None and len(args) != self.arity:
            raise KiteRuntimeError("expected %d arguments but got %d" % (self.arity, len(args)))
        return self.fn(args)


def type_name(v):
    t = type(v)
    if t is int:
        return "int"
    if t is str:
        return "string"
    if t is bool:
        return "bool"
    if v is None:
        return "nil"
    if t is list:
        return "list"
    if t is KMap:
        return "map"
    return "function"


def truthy(v):
    return v is not None and v is not False


# ----------------------------------------------------------------------------------------------- display

def quote(s):
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"').replace("\n", "\\n").replace("\t", "\\t") + '"'


def show(v, nested, active):
    t = type(v)
    if t is str:
        return quote(v) if nested else v
    if t is bool:
        return "true" if v else "false"
    if t is int:
        return str(v)
    if v is None:
        return "nil"
    if t is list or t is KMap:
        if id(v) in active:
            return "[...]" if t is list else "{...}"
        active.add(id(v))
        if t is list:
            text = "[" + ", ".join(show(x, True, active) for x in v) + "]"
        else:
            text = "{" + ", ".join(show(k, True, active) + ": " + show(x, True, active) for k, x in v.items()) + "}"
        active.discard(id(v))
        return text
    if t is Closure:
        return "<fn %s>" % v.proto.name if v.proto.name else "<fn>"
    return "<builtin %s>" % v.name


def display(v):
    """The text print() and str() produce for a value."""
    return show(v, False, set())


# ---------------------------------------------------------------------------------------------- operators

def equal(a, b):
    ta = type(a)
    if ta is not type(b):
        return False
    if ta is list:
        if a is b:
            return True
        return len(a) == len(b) and all(equal(x, y) for x, y in zip(a, b))
    if ta is int or ta is str or ta is bool or a is None:
        return a == b
    return a is b


def int_div(a, b):
    if b == 0:
        raise KiteRuntimeError("division by zero")
    return a // b


def int_mod(a, b):
    if b == 0:
        raise KiteRuntimeError("division by zero")
    return a % b


def int_pow(a, b):
    if b < 0:
        raise KiteRuntimeError("negative exponent")
    return a ** b


ARITH = {
    "-": lambda a, b: a - b,
    "*": lambda a, b: a * b,
    "/": int_div,
    "%": int_mod,
    "**": int_pow,
}
ORDER = {
    "<": lambda a, b: a < b,
    "<=": lambda a, b: a <= b,
    ">": lambda a, b: a > b,
    ">=": lambda a, b: a >= b,
}


def binary(op, a, b):
    ta, tb = type(a), type(b)
    if op == "==":
        return equal(a, b)
    if op == "!=":
        return not equal(a, b)
    if op == "+":
        if ta is tb and (ta is int or ta is str or ta is list):
            return a + b
    elif op in ARITH:
        if ta is int and tb is int:
            return ARITH[op](a, b)
    elif op in ORDER:
        if ta is tb and (ta is int or ta is str):
            return ORDER[op](a, b)
    raise KiteRuntimeError("unsupported operand types for %s: %s and %s" % (op, type_name(a), type_name(b)))


def negate(a):
    if type(a) is not int:
        raise KiteRuntimeError("unsupported operand type for -: %s" % type_name(a))
    return -a


def check_key(key):
    t = type(key)
    if t is not int and t is not str and t is not bool:
        raise KiteRuntimeError("invalid map key: %s" % type_name(key))
    return key


def index_get(obj, i):
    t = type(obj)
    if t is KMap:
        value = obj.get(check_key(i), MISSING)
        if value is MISSING:
            raise KiteRuntimeError("key not found")
        return value
    if t is not list and t is not str:
        raise KiteRuntimeError("cannot index %s" % type_name(obj))
    if type(i) is not int:
        raise KiteRuntimeError("index must be an int")
    n = len(obj)
    if i < 0:
        i += n
    if not 0 <= i < n:
        raise KiteRuntimeError("index out of range")
    return obj[i]


def index_set(obj, i, value):
    if type(obj) is KMap:
        obj.set(check_key(i), value)
        return value
    if type(obj) is not list:
        raise KiteRuntimeError("cannot assign to an element of %s" % type_name(obj))
    if type(i) is not int:
        raise KiteRuntimeError("index must be an int")
    n = len(obj)
    if i < 0:
        i += n
    if not 0 <= i < n:
        raise KiteRuntimeError("index out of range")
    obj[i] = value
    return value


# ----------------------------------------------------------------------------------------------- builtins

def make_builtins(out):
    def b_print(args):
        out.write(" ".join(display(a) for a in args) + "\n")
        return None

    def b_len(args):
        v = args[0]
        if type(v) is KMap:
            return v.count
        if type(v) is not str and type(v) is not list:
            raise KiteRuntimeError("len: expected a string, list or map")
        return len(v)

    def need_map(name, m):
        if type(m) is not KMap:
            raise KiteRuntimeError("%s: expected a map" % name)
        return m

    def b_has(args):
        return need_map("has", args[0]).has(check_key(args[1]))

    def b_del(args):
        found, value = need_map("del", args[0]).remove(check_key(args[1]))
        if not found:
            raise KiteRuntimeError("key not found")
        return value

    def b_keys(args):
        return need_map("keys", args[0]).keys()

    def b_str(args):
        return display(args[0])

    def b_type(args):
        return type_name(args[0])

    def b_push(args):
        if type(args[0]) is not list:
            raise KiteRuntimeError("push: expected a list")
        args[0].append(args[1])
        return None

    def b_pop(args):
        if type(args[0]) is not list:
            raise KiteRuntimeError("pop: expected a list")
        if not args[0]:
            raise KiteRuntimeError("pop: empty list")
        return args[0].pop()

    def b_int(args):
        s = args[0]
        if type(s) is not str:
            raise KiteRuntimeError("int: expected a string")
        digits = s[1:] if s.startswith("-") else s
        if not digits or any(c not in "0123456789" for c in digits):
            raise KiteRuntimeError("int: invalid literal")
        return int(s)

    def b_ord(args):
        s = args[0]
        if type(s) is not str or len(s) != 1:
            raise KiteRuntimeError("ord: expected a string of length 1")
        return ord(s)

    def b_chr(args):
        n = args[0]
        if type(n) is not int:
            raise KiteRuntimeError("chr: expected an int")
        if not 0 <= n <= 127:
            raise KiteRuntimeError("chr: out of range")
        return chr(n)

    table = [("print", None, b_print), ("len", 1, b_len), ("str", 1, b_str), ("type", 1, b_type),
             ("push", 2, b_push), ("pop", 1, b_pop), ("int", 1, b_int), ("ord", 1, b_ord), ("chr", 1, b_chr),
             ("has", 2, b_has), ("del", 2, b_del), ("keys", 1, b_keys)]
    return {name: Builtin(name, arity, fn) for name, arity, fn in table}
