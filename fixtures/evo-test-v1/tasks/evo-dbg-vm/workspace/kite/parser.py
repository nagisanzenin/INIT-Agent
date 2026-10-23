"""Kite parser: a recursive-descent parser from tokens to the syntax tree in nodes.py.

One method per grammar rule of SPEC.md, from the lowest precedence level to the highest.
"""
from . import nodes as N
from .errors import KiteSyntaxError

ASSIGN_OPS = frozenset(["=", "+=", "-=", "*=", "/=", "%="])
EQUALITY_OPS = frozenset(["==", "!="])
COMPARISON_OPS = frozenset(["<", "<=", ">", ">="])
TERM_OPS = frozenset(["+", "-"])
FACTOR_OPS = frozenset(["*", "/", "%"])


class Parser:
    def __init__(self, tokens):
        self.tokens = tokens
        self.pos = 0

    # -------------------------------------------------------------------------------------------- helpers

    def peek(self, ahead=0):
        i = min(self.pos + ahead, len(self.tokens) - 1)
        return self.tokens[i]

    def check(self, kind):
        return self.peek().kind == kind

    def advance(self):
        tok = self.tokens[self.pos]
        if tok.kind != "eof":
            self.pos += 1
        return tok

    def match(self, *kinds):
        if self.peek().kind in kinds:
            return self.advance()
        return None

    def expect(self, kind, what):
        tok = self.peek()
        if tok.kind != kind:
            raise KiteSyntaxError("expected %s" % what, tok.line)
        return self.advance()

    # ------------------------------------------------------------------------------------------ statements

    def program(self):
        stmts = []
        while not self.check("eof"):
            stmts.append(self.declaration())
        return stmts

    def declaration(self):
        if self.check("let"):
            return self.let_decl()
        if self.check("fn") and self.peek(1).kind == "ident":
            return self.fn_decl()
        return self.statement()

    def let_decl(self):
        self.expect("let", "'let'")
        name = self.expect("ident", "a variable name")
        init = None
        if self.match("="):
            init = self.expression()
        self.expect(";", "';' after a declaration")
        return N.Let(name.text, init, name.line)

    def fn_decl(self):
        kw = self.expect("fn", "'fn'")
        name = self.expect("ident", "a function name")
        function = self.function_rest(name.text, kw.line)
        return N.FnDecl(name.text, function, name.line)

    def function_rest(self, name, line):
        self.expect("(", "'(' after fn")
        params = []
        if not self.check(")"):
            while True:
                p = self.expect("ident", "a parameter name")
                params.append((p.text, p.line))
                if not self.match(","):
                    break
        self.expect(")", "')' after the parameters")
        body = self.block()
        return N.Function(name, params, body.stmts, line)

    def statement(self):
        tok = self.peek()
        k = tok.kind
        if k == "if":
            return self.if_stmt()
        if k == "while":
            return self.while_stmt()
        if k == "for":
            return self.for_stmt()
        if k == "return":
            self.advance()
            value = None
            if not self.check(";"):
                value = self.expression()
            self.expect(";", "';' after return")
            return N.Return(value, tok.line)
        if k == "break":
            self.advance()
            self.expect(";", "';' after break")
            return N.Break(tok.line)
        if k == "continue":
            self.advance()
            self.expect(";", "';' after continue")
            return N.Continue(tok.line)
        if k == "{":
            return self.block()
        return self.expr_stmt()

    def expr_stmt(self):
        line = self.peek().line
        expr = self.expression()
        self.expect(";", "';' after an expression")
        return N.ExprStmt(expr, line)

    def block(self):
        open_ = self.expect("{", "'{'")
        stmts = []
        while not self.check("}"):
            if self.check("eof"):
                raise KiteSyntaxError("expected '}'", self.peek().line)
            stmts.append(self.declaration())
        self.advance()
        return N.Block(stmts, open_.line)

    def if_stmt(self):
        kw = self.advance()
        self.expect("(", "'(' after if")
        cond = self.expression()
        self.expect(")", "')' after the condition")
        then = self.block()
        orelse = None
        if self.match("else"):
            if self.check("if"):
                orelse = self.if_stmt()
            else:
                orelse = self.block()
        return N.If(cond, then, orelse, kw.line)

    def while_stmt(self):
        kw = self.advance()
        self.expect("(", "'(' after while")
        cond = self.expression()
        self.expect(")", "')' after the condition")
        body = self.block()
        return N.While(cond, body, kw.line)

    def for_stmt(self):
        kw = self.advance()
        self.expect("(", "'(' after for")
        if self.match(";"):
            init = None
        elif self.check("let"):
            init = self.let_decl()
        else:
            init = self.expr_stmt()
        cond = None
        if not self.check(";"):
            cond = self.expression()
        self.expect(";", "';' after the loop condition")
        step = None
        if not self.check(")"):
            step = self.expression()
        self.expect(")", "')' after the for clauses")
        body = self.block()
        return N.For(init, cond, step, body, kw.line)

    # ----------------------------------------------------------------------------------------- expressions

    def expression(self):
        return self.assignment()

    def assignment(self):
        target = self.logic_or()
        tok = self.peek()
        if tok.kind in ASSIGN_OPS:
            self.advance()
            if not isinstance(target, (N.Name, N.Index)):
                raise KiteSyntaxError("invalid assignment target", tok.line)
            value = self.assignment()
            return N.Assign(target, tok.kind, value, tok.line)
        return target

    def logic_or(self):
        left = self.logic_and()
        while self.check("or"):
            tok = self.advance()
            right = self.logic_and()
            left = N.Logical("or", left, right, tok.line)
        return left

    def logic_and(self):
        left = self.equality()
        while self.check("and"):
            tok = self.advance()
            right = self.equality()
            left = N.Logical("and", left, right, tok.line)
        return left

    def equality(self):
        left = self.comparison()
        while self.peek().kind in EQUALITY_OPS:
            tok = self.advance()
            right = self.comparison()
            left = N.Binary(tok.kind, left, right, tok.line)
        return left

    def comparison(self):
        left = self.term()
        while self.peek().kind in COMPARISON_OPS:
            tok = self.advance()
            right = self.term()
            left = N.Binary(tok.kind, left, right, tok.line)
        return left

    def term(self):
        left = self.factor()
        while self.peek().kind in TERM_OPS:
            tok = self.advance()
            right = self.term()
            left = N.Binary(tok.kind, left, right, tok.line)
        return left

    def factor(self):
        left = self.unary()
        while self.peek().kind in FACTOR_OPS:
            tok = self.advance()
            right = self.unary()
            left = N.Binary(tok.kind, left, right, tok.line)
        return left

    def unary(self):
        tok = self.peek()
        if tok.kind in ("-", "not"):
            self.advance()
            operand = self.unary()
            return N.Unary(tok.kind, operand, tok.line)
        return self.power()

    def power(self):
        base = self.postfix()
        if self.check("**"):
            tok = self.advance()
            exponent = self.unary()
            return N.Binary("**", base, exponent, tok.line)
        return base

    def postfix(self):
        expr = self.primary()
        while True:
            if self.check("("):
                tok = self.advance()
                args = []
                if not self.check(")"):
                    while True:
                        args.append(self.expression())
                        if not self.match(","):
                            break
                self.expect(")", "')' after the arguments")
                expr = N.Call(expr, args, tok.line)
            elif self.check("["):
                tok = self.advance()
                index = self.expression()
                self.expect("]", "']' after the index")
                expr = N.Index(expr, index, tok.line)
            else:
                return expr

    def primary(self):
        tok = self.peek()
        k = tok.kind
        if k == "int" or k == "str":
            self.advance()
            return N.Literal(tok.value, tok.line)
        if k == "true":
            self.advance()
            return N.Literal(True, tok.line)
        if k == "false":
            self.advance()
            return N.Literal(False, tok.line)
        if k == "nil":
            self.advance()
            return N.Literal(None, tok.line)
        if k == "ident":
            self.advance()
            return N.Name(tok.text, tok.line)
        if k == "(":
            self.advance()
            expr = self.expression()
            self.expect(")", "')'")
            return expr
        if k == "[":
            self.advance()
            items = []
            if not self.check("]"):
                while True:
                    items.append(self.expression())
                    if not self.match(","):
                        break
            self.expect("]", "']' after the list items")
            return N.ListLit(items, tok.line)
        if k == "{":
            self.advance()
            pairs = []
            if not self.check("}"):
                while True:
                    key = self.expression()
                    self.expect(":", "':' after a map key")
                    pairs.append((key, self.expression()))
                    if not self.match(","):
                        break
            self.expect("}", "'}' after the map items")
            return N.MapLit(pairs, tok.line)
        if k == "fn":
            self.advance()
            return self.function_rest(None, tok.line)
        raise KiteSyntaxError("unexpected %s" % (repr(tok.text) if tok.text else "end of file"), tok.line)


def parse(tokens):
    return Parser(tokens).program()
