# Kite language specification

Kite is a small dynamically typed scripting language. `python3 run.py PROGRAM.kite` runs one program file
(UTF-8). This document is the complete contract of the interpreter: every program behaves exactly as described
here.

## 1. Running a program

- A program is checked completely before anything runs. If it has a syntax error (section 2, 3) or a static
  error (section 5.4), run.py prints exactly one line `syntax error (line N)` on standard output, where N is the
  line of the offending token, runs nothing, and exits with status 2. (A human-readable message may be written
  to standard error; its text is not part of this contract.)
- Otherwise the program runs. Output of `print` goes to standard output.
- If a runtime error occurs, the output printed so far stays, run.py then prints one line
  `runtime error (line N): MESSAGE` (section 8) on standard output and exits with status 1.
- A program that executes more than 100,000 VM instructions is stopped with the line
  `runtime error: step limit exceeded` (no line number) and exit status 1. Normal programs stay far below this.
- Otherwise run.py exits with status 0.

## 2. Lexical structure

- Whitespace: space, tab, carriage return, newline. Lines are numbered from 1; every newline character
  (also one inside a block comment) starts a new line.
- Comments: `#` up to the end of the line; `/* ... */` block comments (not nested; may span lines). An
  unterminated block comment is a syntax error at the line where it starts.
- Integer literals: one or more decimal digits (`007` is 7). A digit sequence directly followed by a letter or
  `_` is a syntax error. There are no negative literals: `-5` is unary minus applied to 5.
- String literals: double quotes, on one line. Escapes: `\n` (newline), `\t` (tab), `\\`, `\"`. Any other
  escape, a raw newline inside the quotes, or a missing closing quote is a syntax error (at the line where the
  string starts).
- Identifiers: a letter or `_`, then letters, digits or `_` (ASCII only).
- Keywords: `let fn return if else while for break continue true false nil and or not`.
- Operators and punctuation: `+ - * / % ** == != < <= > >= = += -= *= /= %= ( ) { } [ ] , ; :`.
  The longest match wins (`**` is one token, `<=` is one token). Any other character is a syntax error.

## 3. Grammar

```
program     := declaration* EOF
declaration := letDecl | fnDecl | statement
letDecl     := "let" IDENT ( "=" expression )? ";"
fnDecl      := "fn" IDENT "(" params? ")" block
params      := IDENT ( "," IDENT )*
statement   := exprStmt | ifStmt | whileStmt | forStmt | returnStmt
             | "break" ";" | "continue" ";" | block
exprStmt    := expression ";"
ifStmt      := "if" "(" expression ")" block ( "else" ( ifStmt | block ) )?
whileStmt   := "while" "(" expression ")" block
forStmt     := "for" "(" ( letDecl | exprStmt | ";" ) expression? ";" expression? ")" block
returnStmt  := "return" expression? ";"
block       := "{" declaration* "}"
```

Expressions, from the lowest precedence to the highest:

| level | operators | associativity |
|---|---|---|
| assignment | `=  +=  -=  *=  /=  %=` | right |
| or | `or` | left |
| and | `and` | left |
| equality | `==  !=` | left |
| comparison | `<  <=  >  >=` | left |
| term | `+  -` | left |
| factor | `*  /  %` | left |
| unary | prefix `-`, `not` | (prefix) |
| power | `**` | right; see below |
| postfix | call `f(a, b)`, index `x[i]` | left |

- Left associative means `a - b - c` is `(a - b) - c` and `a - b + c` is `(a - b) + c`; likewise
  `a / b * c` is `(a / b) * c`. Comparisons chain the same way: `a < b < c` is `(a < b) < c`.
- `**` binds tighter than a unary operator on its left and its right operand is a unary expression:
  `-2 ** 2` is `-(2 ** 2)` = -4, `2 ** -1` is `2 ** (-1)`, `2 ** 3 ** 2` is `2 ** (3 ** 2)` = 512.
- The target of an assignment must be a variable name or an index expression (`x[i]`, `f()[i][j]`); anything else
  is a syntax error at the assignment operator. Assignment is an expression whose value is the assigned value:
  `a = b = 3;` sets both.
- Primary expressions: integer and string literals, `true`, `false`, `nil`, a name, `( expression )`, a list
  literal `[e1, e2, ...]` (possibly empty), a map literal `{k1: v1, k2: v2, ...}` (possibly empty), and a function
  literal `fn ( params? ) block`. A `{` at the start of a statement always begins a block; anywhere else in an
  expression it begins a map literal. The keys and values of a map literal are evaluated left to right
  (k1, v1, k2, v2, ...) and inserted in that order.
- A statement that starts with `fn` followed by a name is a function declaration; a statement that starts with
  `fn (` is an expression statement holding a function literal.

## 4. Values and types

| type | values | `type(v)` |
|---|---|---|
| int | arbitrary-precision integers | `"int"` |
| string | immutable sequences of characters | `"string"` |
| bool | `true`, `false` | `"bool"` |
| nil | `nil` | `"nil"` |
| list | mutable sequences of values, shared by reference | `"list"` |
| map | mutable maps from keys to values, shared by reference | `"map"` |
| function | functions, closures and builtins | `"function"` |

Truthiness: `false` and `nil` are false; every other value is true (including `0`, `""` and `[]`).

## 5. Variables and scope

### 5.1 Globals
A `let` or `fn` declaration at the top level of the program (not inside any block, function or `for`) declares
a global. Globals are looked up by name when the code that uses them runs, so a function may refer to a global
declared later in the file, as long as it is defined when the reference executes. Declaring an existing global
again simply rebinds it. Reading or assigning a global that is not defined is the runtime error
`undefined variable 'NAME'` (at the line of the name). The builtins (section 7) are predefined globals and may be
redefined.

### 5.2 Locals
Every other declaration is local to the innermost enclosing block. Blocks are `{ ... }` bodies of functions,
`if`/`else`, `while` and `for`, and free-standing `{ ... }` blocks. A `for` statement has its own scope around
the whole loop that holds the variable declared in its initializer; the loop body is a block inside it.
Function parameters live in the function body's outermost block.

- A local is visible from the end of its declaration to the end of its block. The initializer of `let x = e;`
  is evaluated before `x` is declared, so a name `x` inside `e` refers to an outer `x`. A function declaration
  `fn f(...) {...}` declares `f` before its body, so a local function can call itself.
- A name refers to the innermost visible declaration that precedes it in the source text (inner blocks may shadow
  outer names, also those of enclosing functions). A name that matches no visible local declaration in any
  enclosing function refers to a global.
- `let` without an initializer sets the variable to `nil`.

### 5.3 Each declaration execution creates a new variable
Every time a `let` or a local `fn` declaration is executed, it creates a **new** variable. In particular, a `let`
inside a loop body creates a fresh variable on every iteration, and a function that runs twice gets fresh
locals each time. The variable declared in a `for` initializer is created once, when the initializer runs, and
is shared by all iterations of that loop.

### 5.4 Static errors (reported as `syntax error (line N)` before the program runs)
- declaring a name twice in the same block (parameters count as declarations of the body's outermost block;
  globals may be redeclared) - N is the line of the second name;
- `break` or `continue` outside a loop body of the current function - N is the line of the keyword;
- `return` outside a function - N is the line of the keyword.

## 6. Semantics

### 6.1 Functions and closures
- Calling a function with a different number of arguments than it has parameters is the runtime error
  `expected N arguments but got M` (N = number of parameters). Arguments are evaluated left to right after the
  callee.
- A function returns the value of the `return` statement it executes, or `nil` if it ends without one
  (`return;` also returns `nil`).
- A function literal or declaration captures the variables of enclosing functions that it refers to, **by
  reference**: it sees later assignments to them, and its own assignments to them are seen by every other code
  that uses the same variable. Captured variables live as long as some closure refers to them. Each call of a
  function that returns a closure gives the closure its own fresh variables (section 5.3).
- At most 200 calls of Kite functions may be active at the same time (the top-level program does not count).
  A call that would make it 201 is the runtime error `stack overflow`. Builtin calls do not count.

### 6.2 Operators
- `+`: int + int (addition), string + string (concatenation), list + list (a new list holding the elements of
  both). Any other combination is an error.
- `-`, `*`: ints only.
- `/`: integer division of ints that rounds the quotient **toward zero**: `7 / 2` is 3, `-7 / 2` is -3,
  `7 / -2` is -3, `-7 / -2` is 3.
- `%`: the remainder that goes with `/`: `a % b == a - b * (a / b)`, so it has the sign of `a` (or is 0):
  `-7 % 2` is -1, `7 % -2` is 1, `-7 % -2` is -1, `6 % -3` is 0.
- `/` or `%` with a right operand of 0 is the runtime error `division by zero`.
- `**`: int ** int with a non-negative exponent (`0 ** 0` is 1); a negative exponent is the runtime error
  `negative exponent`.
- `<`, `<=`, `>`, `>=`: both ints (numeric order) or both strings (lexicographic order of character code
  points); anything else is an error. The result is a bool.
- `==`, `!=`: defined for any two values; values of different types are never equal (`1 == true` is false).
  Ints, strings and bools compare by value, `nil == nil`, lists compare element by element (same length and
  pairwise `==`), maps and functions are equal only to themselves.
- Unary `-`: ints only. `not x`: `true` if x is false or nil, else `false`.
- `a and b`: evaluates `a`; if it is false or nil the result is `a` itself and `b` is not evaluated, otherwise
  the result is the value of `b`. `a or b`: evaluates `a`; if it is truthy the result is `a` itself and `b` is
  not evaluated, otherwise the result is the value of `b`. (So `nil or 5` is 5, `0 or 5` is 0, `false and x` is
  false, `1 and "x"` is "x".)
- Operand type errors: `unsupported operand types for OP: T1 and T2` for binary operators (OP is the operator,
  T1/T2 the `type()` names of the left and right operands, e.g. `unsupported operand types for -: string and
  int`), `unsupported operand type for -: T` for unary minus.
- Operands are evaluated left to right; both operands of a binary operator are evaluated before it is applied.

### 6.3 Lists and strings
- `x[i]`: x is a list or a string, i an int (maps: section 6.4). Negative indexes count from the end (`x[-1]` is the last element).
  After that adjustment the index must satisfy 0 <= i < len(x). Indexing a string gives a one-character string.
- `x[i] = v` replaces an element of a list (same index rules) and has the value v.
- Errors: `cannot index T` (x is not a list, string or map), `index must be an int`, `index out of range`,
  `cannot assign to an element of T` (assigning into anything but a list or map).

### 6.4 Maps
- Keys must be ints, strings or bools; using any other value as a key (in a literal, an index, `has` or `del`) is
  the runtime error `invalid map key: T`. Keys are compared with `==` of section 6.2, so `1` and `true` are two
  different keys, as are `1` and `"1"`.
- `m[k]` is the value stored under k; if k is not in the map it is the runtime error `key not found`.
- `m[k] = v` stores v under k. A new key is added at the end of the map's order; storing under an existing key
  replaces its value and keeps its position. `m[k] op= e` reads `m[k]` (so k must be present) and stores the result.
- `del(m, k)` removes k (error `key not found` if absent) and returns its value. A removed key that is stored
  again is a new key: it goes to the end of the order.
- The order of a map is the order in which its present keys were added; `keys(m)`, `len(m)` and the display
  form follow it. A map may hold any number of entries; every operation behaves the same whatever the sequence
  of earlier insertions and removals was.

### 6.5 Assignment
- `x = e` assigns to the variable `x` resolved as in section 5 (local, captured, or global).
- `x op= e` (for `+= -= *= /= %=`) means `x = x op e`: the current value of `x` is the **left** operand.
  `t[i] op= e` evaluates `t` and `i` once, then does `t[i] = t[i] op e`.

### 6.6 Statements
- `if`/`else` choose a branch by truthiness; `else if` chains are allowed.
- `while (c) { body }` repeats while `c` is truthy.
- `for (init; cond; step) { body }`: runs `init` once; then, as long as `cond` is truthy (a missing condition is
  always true), runs the body and then the step expression (if any).
- `break` leaves the innermost enclosing loop. `continue` ends the current iteration of the innermost enclosing
  loop: in a `while` loop it goes to the condition test; in a `for` loop it goes to the **step** expression,
  then the condition test.

## 7. Builtins

| builtin | behaviour |
|---|---|
| `print(v1, ..., vn)` | writes the display forms of the values separated by one space, then a newline (`print()` writes an empty line); returns nil |
| `len(x)` | number of elements of a list, characters of a string or entries of a map; otherwise error `len: expected a string, list or map` |
| `str(v)` | the display form of v as a string |
| `type(v)` | the type name (section 4) |
| `push(l, v)` | appends v to list l, returns nil; error `push: expected a list` if l is not a list |
| `pop(l)` | removes and returns the last element; errors `pop: expected a list`, `pop: empty list` |
| `int(s)` | parses a string of decimal digits with an optional leading `-` (e.g. `"-042"` is -42); errors `int: expected a string` (not a string), `int: invalid literal` (anything else, e.g. `""`, `"+1"`, `"1 "`) |
| `ord(s)` | the code point of a one-character string; otherwise error `ord: expected a string of length 1` |
| `chr(n)` | the one-character string with code point n, for 0 <= n <= 127; errors `chr: expected an int`, `chr: out of range` |
| `has(m, k)` | `true` if key k is in map m, else `false`; errors `has: expected a map`, `invalid map key: T` |
| `del(m, k)` | removes key k from map m and returns its value (section 6.4); errors `del: expected a map`, `invalid map key: T`, `key not found` |
| `keys(m)` | a new list of the keys of map m, in the map's order; error `keys: expected a map` |

Builtins check their argument count like functions do (`expected N arguments but got M`); `print` accepts any
number of arguments. Errors inside a builtin are reported at the line of the call (section 8).

### Display form
- int: decimal, with a leading `-` if negative. bool: `true` / `false`. nil: `nil`.
- string: its characters as they are (no quotes) - except inside a list or map, see below.
- list: `[` + the elements' list forms joined by `, ` + `]`; `[]` when empty. In a list or map, a string is
  shown in double quotes with `\` `"` newline and tab written as `\\` `\"` `\n` `\t`; other values as above.
- map: `{` + `KEY: VALUE` pairs (both in list form) in the map's order joined by `, ` + `}`; `{}` when empty.
- A list or map that is being displayed and is reached again inside itself (it contains itself, directly or
  indirectly) shows that inner reference as `[...]` (list) or `{...}` (map). The same list or map appearing
  several times side by side, without containing itself, is displayed in full each time.
- function: `<fn NAME>` for a function declared with a name, `<fn>` for a function literal,
  `<builtin NAME>` for a builtin.

## 8. Runtime errors

The line N in `runtime error (line N): MESSAGE` is the line of the token that performed the failing operation:
- the operator token for unary and binary operators and for compound assignments (`+=` ...);
- the `(` of the call for calls (wrong argument count, calling a non-function, stack overflow, and every error
  raised inside a builtin);
- the `[` for indexing (`x[i]` reads; for `x[i] = v` the `=` token);
- the `{` of a map literal for an invalid key in it;
- the name for undefined variables.

Messages (exact text):
`undefined variable 'NAME'`, `unsupported operand types for OP: T1 and T2`, `unsupported operand type for -: T`,
`division by zero`, `negative exponent`, `cannot call T`, `expected N arguments but got M`, `stack overflow`,
`cannot index T`, `index must be an int`, `index out of range`, `cannot assign to an element of T`,
`invalid map key: T`, `key not found`, and the builtin messages of section 7.
