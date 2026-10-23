"""Kite virtual machine: runs the bytecode of compiler.py.

Every call gets a Frame with its own operand stack and its own slots (parameters first, then the other locals of
the function). A captured local holds a Cell in its slot; closures keep a list of the cells they captured.
Global reads are cached per instruction, tagged with the `version` of the globals table at the time of the read.
"""
from .errors import KiteRuntimeError
from .kmap import KMap
from .values import Cell, Closure, Builtin, truthy, binary, negate, index_get, index_set, type_name, make_builtins

MAX_CALL_DEPTH = 200
STEP_LIMIT = 100000


class StepLimitExceeded(KiteRuntimeError):
    def __init__(self):
        KiteRuntimeError.__init__(self, "step limit exceeded")

    def report(self):
        return "runtime error: step limit exceeded"


class Frame:
    __slots__ = ("closure", "code", "lines", "consts", "slots", "stack", "ip")

    def __init__(self, closure, slots):
        proto = closure.proto
        self.closure = closure
        self.code = proto.code
        self.lines = proto.lines
        self.consts = proto.consts
        self.slots = slots
        self.stack = []
        self.ip = 0


class VM:
    def __init__(self, out):
        self.out = out
        self.globals = make_builtins(out)

    def run(self, main_proto):
        frame = Frame(Closure(main_proto, []), [None] * main_proto.nslots)
        try:
            self.execute(frame)
        finally:
            self.out.flush()

    def execute(self, frame):
        frames = [frame]
        globals_ = self.globals
        code, stack, slots, ip = frame.code, frame.stack, frame.slots, 0
        gcache = frame.closure.proto.global_cache
        version = 0
        steps = 0
        try:
            while True:
                steps += 1
                if steps > STEP_LIMIT:
                    raise StepLimitExceeded()
                op, arg = code[ip]
                ip += 1

                if op == "CONST":
                    stack.append(frame.consts[arg])
                elif op == "LOAD_LOCAL":
                    stack.append(slots[arg])
                elif op == "SET_LOCAL":
                    slots[arg] = stack.pop()
                elif op == "STORE_LOCAL":
                    slots[arg] = stack[-1]
                elif op == "LOAD_CELL":
                    stack.append(slots[arg].value)
                elif op == "STORE_CELL":
                    slots[arg].value = stack[-1]
                elif op == "MAKE_CELL":
                    slots[arg] = Cell(stack.pop())
                elif op == "LOAD_FREE":
                    stack.append(frame.closure.cells[arg].value)
                elif op == "STORE_FREE":
                    frame.closure.cells[arg].value = stack[-1]
                elif op == "LOAD_GLOBAL":
                    hit = gcache.get(ip)
                    if hit is not None and hit[0] == version:
                        stack.append(hit[1])
                    else:
                        if arg not in globals_:
                            raise KiteRuntimeError("undefined variable '%s'" % arg)
                        value = globals_[arg]
                        gcache[ip] = (version, value)
                        stack.append(value)
                elif op == "STORE_GLOBAL":
                    if arg not in globals_:
                        raise KiteRuntimeError("undefined variable '%s'" % arg)
                    globals_[arg] = stack[-1]
                    version += 1
                elif op == "DEFINE_GLOBAL":
                    globals_[arg] = stack.pop()
                elif op == "POP":
                    stack.pop()
                elif op == "NIL":
                    stack.append(None)
                elif op == "TRUE":
                    stack.append(True)
                elif op == "FALSE":
                    stack.append(False)
                elif op == "DUP2":
                    stack.extend(stack[-2:])
                elif op == "BINARY":
                    b = stack.pop()
                    a = stack.pop()
                    stack.append(binary(arg, a, b))
                elif op == "NEG":
                    stack.append(negate(stack.pop()))
                elif op == "NOT":
                    stack.append(not truthy(stack.pop()))
                elif op == "JUMP":
                    ip = arg
                elif op == "JUMP_IF_FALSE":
                    if not truthy(stack.pop()):
                        ip = arg
                elif op == "JUMP_IF_FALSE_OR_POP":
                    if truthy(stack[-1]):
                        stack.pop()
                    else:
                        ip = arg
                elif op == "JUMP_IF_TRUE_OR_POP":
                    if truthy(stack[-1]):
                        ip = arg
                    else:
                        stack.pop()
                elif op == "BUILD_LIST":
                    start = len(stack) - arg
                    items = stack[start:]
                    del stack[start:]
                    stack.append(items)
                elif op == "BUILD_MAP":
                    start = len(stack) - 2 * arg
                    m = KMap()
                    for j in range(start, len(stack), 2):
                        index_set(m, stack[j], stack[j + 1])
                    del stack[start:]
                    stack.append(m)
                elif op == "INDEX_GET":
                    i = stack.pop()
                    obj = stack.pop()
                    stack.append(index_get(obj, i))
                elif op == "INDEX_SET":
                    value = stack.pop()
                    i = stack.pop()
                    obj = stack.pop()
                    stack.append(index_set(obj, i, value))
                elif op == "CLOSURE":
                    proto = frame.consts[arg]
                    cells = []
                    for is_local, index in proto.free:
                        cells.append(slots[index] if is_local else frame.closure.cells[index])
                    stack.append(Closure(proto, cells))
                elif op == "CALL":
                    base = len(stack) - arg
                    callee = stack[base - 1]
                    if type(callee) is Closure:
                        proto = callee.proto
                        if arg != proto.nparams:
                            raise KiteRuntimeError("expected %d arguments but got %d" % (proto.nparams, arg))
                        if len(frames) > MAX_CALL_DEPTH:
                            raise KiteRuntimeError("stack overflow")
                        new_slots = stack[base:]
                        new_slots.extend([None] * (proto.nslots - arg))
                        del stack[base - 1:]
                        frame.ip = ip
                        frame = Frame(callee, new_slots)
                        frames.append(frame)
                        code, stack, slots, ip = frame.code, frame.stack, frame.slots, 0
                        gcache = proto.global_cache
                    elif type(callee) is Builtin:
                        args = stack[base:]
                        del stack[base - 1:]
                        stack.append(callee.call(args))
                    else:
                        raise KiteRuntimeError("cannot call %s" % type_name(callee))
                elif op == "RETURN":
                    result = stack.pop()
                    frames.pop()
                    if not frames:
                        return result
                    frame = frames[-1]
                    code, stack, slots, ip = frame.code, frame.stack, frame.slots, frame.ip
                    gcache = frame.closure.proto.global_cache
                    stack.append(result)
                else:
                    raise RuntimeError("unknown opcode %r" % op)
        except KiteRuntimeError as e:
            if e.line is None and not isinstance(e, StepLimitExceeded):
                e.line = frame.lines[ip - 1]
            raise
