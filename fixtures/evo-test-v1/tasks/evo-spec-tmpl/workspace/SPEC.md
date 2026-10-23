# Stencil 1.1: template language specification

Stencil renders a template (a text with tags) using a context (a dict of values). This file is the contract
for the `stencil` package and its command line. Every rule is numbered (R1-R64) and every rule is tested.

Stencil looks a little like Jinja, Liquid and Mustache but it is NOT any of them. Many rules deliberately
differ from those engines (delimiters, `end`, falsy values, escaping, `??`, loop variables, scoping, macros,
map iteration order, whitespace control, comments). When your intuition from another engine disagrees with
this file, this file wins.

Notation: `\n` is a newline (LF) and `\t` a tab. "Whitespace" always means exactly these four characters:
space, `\t`, `\n`, `\r`. Examples show a template, an arrow, and the rendered text in quotes; `ERROR` shows
the exact `str()` of the raised error.

## 1. API

**R1. render.** `stencil.render(template, context)` takes the template as a `str` and the context as a `dict`
with `str` keys, and returns the rendered text as a `str`. Context values are `str`, `int`, `bool`, `None`,
`list` and `dict` (with `str` keys), nested to any depth. Other Python types are not supported. `render` never
modifies `context`.

**R2. Errors and types.** Every error is raised as `stencil.TemplateError` (a subclass of `Exception`) with
attributes `line` and `col` (ints, 1-based, R61) and `message` (the exact text listed in this file).
`str(err)` is exactly `"<line>:<col>: <message>"`, for example `3:14: undefined variable 'user.name'`.
When an error is raised, no output is returned (there is no partial result).
Stencil has six value types. Their names, used in error messages, are: `string` (Python `str`), `int`,
`bool`, `none` (Python `None`), `list` and `map` (Python `dict`). A Python `bool` is always type `bool`,
never `int` (so `true` is not equal to `1`, and filters that take an `int` reject a `bool`).

## 2. Lexical structure

**R3. Constructs.** The lexer scans the template from left to right and splits it into:

| construct      | opens with | ends at                                    |
|----------------|------------|--------------------------------------------|
| output tag     | `{=`       | the FIRST `=}` after the opener (R4)       |
| statement tag  | `{:`       | the FIRST `:}` after the opener (R4)       |
| comment        | `{#`       | the matching `#}` (comments nest, R5)      |
| verbatim block | see R6     | see R6                                     |
| text           | anything else, emitted as it is (apart from whitespace control, R8-R13) | |

A `{` that is not followed by `=`, `:` or `#` is text, and so are `}`, `=}`, `:}` and `#}` found in text.
In particular the Jinja forms `{{ name }}` and `{% if x %}` are plain text in Stencil.

**R4. Tag ends.** Because a tag ends at the first closing delimiter, a string literal cannot contain the
closing delimiter of its own tag: `{= "a=}" =}` is the output tag `{= "a=}` (whose content `"a` is an
unterminated string, R14) followed by the text `" =}`. A string in an output tag may contain `:}` and one in
a statement tag may contain `=}`.

**R5. Comments.** A comment produces no output. Comments nest: inside a comment every `{#` opens a nested
comment and every `#}` closes the innermost open one; the comment ends when the outermost one is closed.
Everything inside a comment (including `{=`, `{:` and unbalanced quotes) is ignored.

    a{# one {# two #} still comment #}b        ->  "ab"
    a{# {= broken :} #}b                       ->  "ab"

Comments take no trim markers: `{#- x -#}` is simply a comment whose content is `- x -`.

**R6. Verbatim blocks.** A statement tag whose content (after removing its trim markers, R7, and surrounding
whitespace) is exactly `verbatim` opens a verbatim block. The block's content runs up to the first closing
tag, which is: `{:`, an optional marker, optional whitespace, `/verbatim`, optional whitespace, an optional
marker, `:}` (so `{:/verbatim:}`, `{: /verbatim :}` and `{:- /verbatim ~:}` all close it). The content is
emitted exactly as written: no tags, comments or escaping are processed inside it, and verbatim blocks do not
nest (an inner `{: verbatim :}` is just content). For whitespace control (R8-R13) the content is a text
segment of its own, and the opening and closing tags are constructs like statement tags.

    {: verbatim :}{= x =} {# y #}{: /verbatim :}   ->  "{= x =} {# y #}"

A `{: /verbatim :}` tag outside a verbatim block is an ordinary statement tag, which fails with
`unexpected character '/'` (R15).

**R7. Trim markers.** The character immediately after `{=` or `{:` may be a left marker, and the character
immediately before `=}` or `:}` may be a right marker. The marker characters are `-` and `~`. Markers are
not part of the tag content. The left marker is taken first, then the right marker from what remains:
`{=-3=}` has a left marker and the content `3`; `{= -3 =}` has no marker and the content ` -3 ` (the int
-3); `{=--=}` has two markers and an empty content.

## 3. Whitespace control

**R8. Standalone lines.** A statement tag, a comment, or a verbatim opening or closing tag that has NO trim
marker is *standalone* when, in the template source as written, the characters between the previous `\n` (or
the start of the template) and the construct are only spaces and tabs (or nothing), and the characters
between the end of the construct and the next `\n` (or the end of the template) are only spaces and tabs (or
nothing). In other words it is alone on its line. Output tags are never standalone, and a construct with any
trim marker is never standalone.

**R9. Standalone removal.** For every standalone construct, the spaces and tabs before it on its line and
the spaces and tabs after it on its line, plus the `\n` that ends the line (if there is one), are removed.
All constructs are judged on the source as written before anything is removed (so consecutive standalone
lines are all removed). Lines are judged on the source text, so a line that also contains part of a
multi-line comment or a `{=` tag is not standalone.

    "<ul>\n  {: for x in xs :}\n  <li>{= x =}</li>\n  {: end :}\n</ul>"  with xs = [1, 2]
        ->  "<ul>\n  <li>1</li>\n  <li>2</li>\n</ul>"
    "a\n{# note #}\nb"                  ->  "a\nb"
    "a {: if t :}b{: end :}\n"          ->  "a b\n"          (not alone on its line)

**R10. Trim scope.** A left marker trims the END of the text segment immediately before its tag; a right
marker trims the START of the text segment immediately after its tag. A text segment is the text between
two consecutive constructs (tags, comments, verbatim tags), or between a construct and the start or end of
the template. Trimming never goes past a construct: a comment between a tag and some whitespace protects
that whitespace. Whitespace control is applied once to the template source before rendering, so it does
not depend on which branches, iterations or macro calls run. When one segment is trimmed on both sides by
markers, its start is trimmed first, then its end.

**R11. `-` trims to the line.** A left `-` removes the spaces and tabs at the end of the preceding segment
(it never removes a newline). A right `-` removes the spaces and tabs at the start of the following segment
and then, if the next character is `\n`, exactly that one newline.

    "a  \n  {=- x =}"   with x = 1         ->  "a  \n1"
    "{= x -=}  \n\nb"   with x = 1         ->  "1\nb"
    "{= x -=} \t b"     with x = 1         ->  "1b"

**R12. `~` trims all whitespace.** A left `~` removes all whitespace at the end of the preceding segment, a
right `~` all whitespace at the start of the following segment.

    "a \n\t {=~ x ~=} \n\n b"  with x = 1  ->  "a1b"

**R13. Order.** Standalone removal (R8-R9) happens first; then the markers (R10-R12) trim the segments as
they are after the standalone removal.

## 4. Expressions

**R14. Literal tokens.** Inside a tag, tokens are separated by optional whitespace.

* int: an optional `-` immediately followed by decimal digits: `42`, `-7`, `007` (= 7).
* string: double quotes only. Escapes: `\\` (backslash), `\"` (quote), `\n` (newline), `\t` (tab). Any other
  backslash sequence is the error `bad escape '\X'` (X is the character after the backslash); a string
  without its closing quote is `unterminated string`. Single quotes do not delimit strings.

**R15. Other tokens.**

* path: a name followed by zero or more `.segment`; a name is `[A-Za-z_][A-Za-z0-9_]*` and a segment is a
  name or decimal digits. `user.tags.0.label` is one token; there is no whitespace around dots.
* loop variable: `#` immediately followed by a name, e.g. `#index` (R36-R37).
* keywords: the words `and`, `or`, `not`, `in`, `true`, `false`, `none` (exactly these, without dots) are
  keywords, never variable names.
* operators: `|`, `??`, `==`, `!=`, `<=`, `>=`, `<`, `>`, `=`, `,` (two-character operators are matched
  first).

Tokens are read greedily from left to right. Any other character (for example `'`, `.`, `!`, `(`, `{`, or a
`-` or `#` not followed by what the rules above require) is the error `unexpected character 'X'`.

**R16. Grammar.**

    output tag     := expr
    statement tag  := "if" cond | "elif" cond | "else" | "end"
                    | "for" NAME "in" expr | "for" NAME "," NAME "in" expr
                    | "break" | "continue"
                    | "set" NAME "=" expr | "capture" NAME
                    | "macro" NAME param* | "call" NAME atom*
                    | "verbatim"
    param          := NAME [ "=" literal ]
    cond           := and_c ( "or" and_c )*
    and_c          := not_c ( "and" not_c )*
    not_c          := "not" not_c | expr [ cmpop expr ]
    cmpop          := "==" | "!=" | "<" | ">" | "<=" | ">=" | "in"
    expr           := atom ( "??" atom )* ( "|" FILTER atom* )*
    atom           := literal | PATH | LOOPVAR
    literal        := INT | STRING | "true" | "false" | "none"
    NAME, FILTER   := a path token without dots

**R17. Statement words.** The statement words (`if elif else end for break continue set capture macro call
verbatim`) are special only as the first token of a statement tag; anywhere else they are ordinary names
(`{= end =}` outputs the variable `end`). Output tags take a plain expression: comparisons and
`and`/`or`/`not` exist only in `if`/`elif` conditions.

**R18. Filter arguments.** Filter arguments (and macro call arguments) are atoms separated by whitespace (no
parentheses, no commas, no nested filters); an argument list ends at the first token that is not an atom.
So `x | pad 5 "*" | upper` is `pad` with two arguments followed by `upper`, and `{: if x | pad 3 and y :}`
gives `pad` one argument.

**R19. `??` binds tighter than `|`.** `??` may appear only before the first `|` of an expression:
`a ?? b | upper` means `(a ?? b) | upper`, the filters apply to whichever operand was chosen. A `??` after a
filter is a syntax error: `{= x | upper ?? "y" =}` fails with `unexpected token '??'`.

**R20. Syntax error messages.** A tag that contains no token is `empty tag`. If the first token of a
statement tag is not a statement word, the message is `unknown statement 'T'` (T is the token as written).
Otherwise the reported problem is the first token, from left to right, at which the tag stops being the
beginning of some valid tag: the message is `unexpected token 'T'` with T that token exactly as written
(strings with their quotes), or `unexpected end of tag` if the tokens ran out first.

    {: if :}                 unexpected end of tag
    {= a b =}                unexpected token 'b'
    {: else x :}             unexpected token 'x'
    {: endif :}              unknown statement 'endif'
    {: for x xs :}           unexpected token 'xs'
    {= a | "s" =}            unexpected token '"s"'
    {: set x.y = 1 :}        unexpected token 'x.y'
    {= a == b =}             unexpected token '=='
    {: macro m a=b :}        unexpected token 'b'

## 5. Values and lookup

**R21. Paths.** The first part of a path is resolved by R22. Each further segment is applied to the current
value: on a map, the segment is a key (also when it is made of digits: `m.0` reads the key `"0"`); on a
list, a digits segment is a 0-based index; in every other case (a missing key, an index past the end, a
name segment on a list, any segment on a string, int, bool or none) the whole path is missing. A key that
exists with the value none is NOT missing: its value is none.

**R22. Names.** A name is looked up in the scopes from the innermost outwards (R40) and finally in the
context. Inside a macro body only the macro's own scopes come before the context (R46). Loop variables
(`#index`, ...) refer to the innermost running loop iteration of the current template level or macro body
(R36); outside of every such iteration they are missing.

**R23. Missing values.** What a missing path means depends on where it is evaluated:

* in an output tag, a `set` statement and the arguments of a `call` (atoms and filter arguments alike): the
  error `undefined variable 'P'`, where P is the path exactly as written (e.g. `user.name`, `#index`), except
  for operands of `??` covered by R24;
* in an `if`/`elif` condition and in the iterable expression of a `for` (atoms and filter arguments alike):
  the value none, without error.

**R24. Default operator.** `x1 ?? x2 ?? ... ?? xn` evaluates its operands from left to right and takes the
first one that is neither missing nor none; the later operands are not evaluated. Only missing and none are
replaced: `0`, `""`, `false` and empty lists are kept. If every operand is missing or none, the result is
the last operand's value: none if it is none, and if it is missing R23 applies to the last operand (in an
output tag: `undefined variable` naming the last operand).

    {= count ?? 5 =}        with count = 0          ->  "0"
    {= nick ?? name =}      with nick = None, name = "Al"  ->  "Al"
    {= a ?? b =}            with neither defined    ->  ERROR  1:1: undefined variable 'b'

## 6. Output

**R25. Formatting.** The value of an output tag is turned into text as follows: string: as is; int: decimal
(with `-` when negative, no grouping); bool: `true` or `false`; none: the empty string; list: each item
formatted by these rules, joined with `, ` (comma, space), so nested lists flatten
(`[[1, 2], 3]` gives `1, 2, 3`); map: the error `cannot output a map`.

**R26. Escaping.** The formatted text of every output tag is escaped by replacing exactly these six
characters: `&` by `&amp;`, `<` by `&lt;`, `>` by `&gt;`, `"` by `&quot;`, `{` by `&#123;` and `}` by
`&#125;`. Nothing else is replaced; in particular the single quote `'` is left as it is. Escaping applies to
whatever the tag outputs (`{= "<" =}` gives `&lt;`); template text, verbatim content and the text a macro
call inserts (R47) are never escaped.

**R27. safe.** When the last filter of an output tag is `safe`, its text is emitted without escaping. `safe`
takes no argument and must be the last filter of its expression: a filter after it is the syntax error
`'safe' must be the last filter`. In other places (`set`, conditions, `for`) a final `safe` just returns its
input; safeness is not stored in variables, so after `{: set h = x | safe :}` the output `{= h =}` is
escaped.

## 7. Conditions

**R28. Truth.** In a condition every value is true or false by this table (it differs from Python):

| value             | false when                        | true examples               |
|-------------------|-----------------------------------|-----------------------------|
| none, missing     | always false                      |                             |
| bool              | `false`                           | `true`                      |
| int               | `0`                               | `-1`, `7`                   |
| string            | `""` and `"0"` (exactly these)    | `" "`, `"false"`, `"00"`    |
| list              | never: every list is TRUE         | `[]`, `[0]`                 |
| map               | never: every map is TRUE          | `{}`                        |

**R29. Equality.** `==` is true when both values have the same type and are equal (lists and maps compare
item by item with this same rule); otherwise false: `1 == true` is false, `"1" == 1` is false,
`none == none` is true (and a missing value is none in conditions, R23). `!=` is the negation of `==`.

**R30. Ordering.** `<`, `>`, `<=` and `>=` need two ints or two strings (strings compare by code point); any
other pair is the error `cannot compare A and B` where A and B are the type names of the left and right
values. Both sides of any comparison are evaluated (left first) before they are compared.

**R31. Membership.** `a in b` depends on the type of `b`:

* list: true when some item of `b` equals `a` by R29 (so `1 in [true]` is false);
* map: true when `a` is a string and a key of `b`; false for any other `a` (no error);
* string: `a` must be a string, else the error `cannot test membership of T in string` (T is the type of
  `a`); true when `a` occurs in `b` (the empty string occurs in every string);
* none (also a missing `b`): false;
* int or bool: the error `cannot test membership in T` (T is the type of `b`).

A comparison holds one operator only, and `not` binds looser than comparisons: `not a in b` is
`not (a in b)`.

**R32. Logic.** `not` binds tighter than `and`, which binds tighter than `or`. Evaluation goes from left to
right and stops early: `and` at the first false operand, `or` at the first true one. Operands that are not
evaluated cannot raise errors.

## 8. Blocks

**R33. if.** `{: if c :}` ... any number of `{: elif c :}` ... an optional `{: else :}` ... `{: end :}`.
The first branch whose condition is true is rendered, else the `else` branch if any. Conditions after the
first true one are not evaluated. A single `end` closes the innermost open block (`if`, `for`, `capture` or
`macro`); there is no `endif` or `endfor`.

**R34. for.** `{: for x in expr :}` iterates over the value of `expr`:

* list: its items in order;
* map: its keys in ascending code point order (NOT insertion order); with two names, `{: for k, v in m :}`
  binds each key and its value, in the same order;
* none (including a missing iterable, R23): zero iterations;
* string, int, bool: the error `cannot iterate over T` (T is the type name). Strings are not iterable.

Two names over a list (of any length) is the error `two loop variables need a map`.

**R35. Loop else.** `{: for ... :}body{: else :}other{: end :}` renders `other` exactly when the loop has zero
iterations (empty list, empty map, none or missing). It does not depend on `break`. The `else` branch is not
part of an iteration: it runs in the scope that encloses the loop.

**R36. Loop variables.** In an iteration, `#index` is the iteration number starting at 1, `#count` the total
number of items, `#first` is true on the first iteration only and `#last` true on the iteration of the last
item only. In nested loops they refer to the innermost loop. In a loop's `else` branch, and after the loop,
they refer to the enclosing loop iteration if there is one; outside of every iteration they are missing. A
macro body does not see the loops of its caller (R46). Any `#name` not listed in R36-R37 is the syntax
error `unknown loop variable '#name'`.

**R37. More loop variables.** `#rindex` is `#count - #index + 1` (so 1 on the last item). `#depth` is the
number of loops running at this point in the current template level or macro body (1 in an outermost loop,
2 in a loop inside it, ...). `#prev` is the item of the previous iteration and `#next` the item of the next
one, none on the first and on the last item respectively; for a map loop the item is the key. These are
computed from the whole sequence, whether or not `break` is used.

**R38. break and continue.** `{: break :}` ends the innermost loop at once: the rest of the current iteration
is skipped and no further iterations run. `{: continue :}` skips the rest of the current iteration only.
Output already produced stays, and `#count`, `#last`, `#rindex` and `#next` keep their R36-R37 values.

**R39. Where break and continue may appear.** Going outwards from the `break`/`continue` tag and skipping
enclosing `if` blocks, the first enclosing block must be a `for` and the tag must be in that loop's body, not
in its `else` branch. Otherwise (top level, directly inside a `capture` or `macro` body, or in a loop's
`else` branch) it is the structure error `unexpected 'break'` or `unexpected 'continue'` (R60).

**R40. Scopes.** There is one template scope, and every iteration of a loop body gets a new iteration scope
holding the loop variable(s) and every variable set during that iteration (also inside nested `if` blocks).
The iteration scope is discarded at the end of the iteration: afterwards, and after the loop, names refer
again to what they referred to before. `set` and `capture` always write the innermost scope: inside an
iteration they create or overwrite a variable of that iteration (shadowing an outer one, never changing it);
outside of every iteration they write the template scope (or the macro scope in a macro body, R46), visible
from there on, including inside later loops.

    {: set n = 0 :}{: for x in xs :}{: set n = x :}{= n =}{: end :}/{= n =}   with xs = [1, 2]  ->  "12/0"

**R41. set.** `{: set name = expr :}` evaluates `expr` like an output tag does (missing values are errors,
R23) and binds the value to `name` (R40). It outputs nothing. Loop variable names and dotted names cannot be
set (grammar, R16).

**R42. capture.** `{: capture name :}` ... `{: end :}` renders its body where it stands (the body uses and
changes the current scopes like any code at that place; `capture` creates no scope), but instead of being
output the rendered text is bound as a string to `name` in the innermost scope, like `set`. The captured
text has already been escaped by the output tags inside the body, so outputting the variable escapes it a
second time unless `| safe` is used:

    {: capture c :}{= "&" =}{: end :}{= c =}|{= c | safe =}   ->  "&amp;amp;|&amp;"

A `capture` body cannot have `else` or `elif` (`unexpected 'else'`, `unexpected 'elif'`).

## 9. Macros

**R43. Definitions.** `{: macro NAME params :}` ... `{: end :}` defines a macro. It outputs nothing where it
stands. All macros of the template are known before rendering starts, so a macro can be called before (or
after) its definition. A `macro` tag inside any block (including another macro) is the structure error
`macro must be at the top level`; defining a name a second time is the structure error
`macro 'NAME' is already defined` (at the second definition). Macro names and variable names are separate.

**R44. Parameters.** Each parameter is a name, optionally followed by `=` and a literal default (an int, a
string, `true`, `false` or `none`; a path is not allowed as a default). Syntax errors (phase 2, checked after
the grammar, the first by token position): a parameter name used twice is `duplicate parameter 'p'` (at the
second one); a parameter without a default after a parameter with one is `parameter 'p' needs a default`.

**R45. Calls.** `{: call NAME args :}` renders the macro body with the parameters bound to the argument
values in order; parameters without an argument take their default. At the call tag, in this order: the
arguments are evaluated from left to right like output-tag atoms (R23: a missing path is
`undefined variable`); more arguments than parameters is `macro 'NAME' takes at most K arguments, got M`;
fewer arguments than parameters without default is `macro 'NAME' needs at least K arguments, got M` (K and M
are plain decimal numbers; the word is always `arguments`); then R48 is checked.

**R46. Macro scope.** A macro body runs with a fresh scope chain: a new macro scope holding its parameters,
then the context. It does NOT see the caller's template scope, iteration scopes, `set`/`capture` variables or
loop variables: loop variables in a macro body refer only to loops inside that body (outside them they are
missing even when the call stands inside a loop) and `#depth` counts only loops inside the body. `set` and
`capture` in the body (outside its loops) write the macro scope, which is discarded when the call ends, so a
macro never changes its caller's variables. Each call gets a new macro scope.

**R47. Inserted text.** The text the macro body renders is inserted at the call tag unchanged: it is not
escaped again (each output tag inside the body did its own escaping). Whitespace control is applied to the
body's text once, where the body is written (R10); markers on the call tag trim the caller's text around the
call tag, and a call tag alone on its line is standalone (R8).

**R48. Nesting.** A macro may call itself and other macros. A call made while 8 macro calls are already
running is the error `macro calls nested too deeply` (at that call tag).

**R49. Unknown macros.** A `call` of a name that no `macro` defines is the structure error
`unknown macro 'NAME'`. It is checked after all other structure checks (R60) and the first such call tag in
template order is reported, whether or not the call would ever be rendered.

## 10. Filters

**R50. Applying filters.** Filters apply from left to right. For each filter: first its arguments are
evaluated from left to right (a missing argument is handled by R23), then the type of the input value is
checked, then the arguments are checked from left to right. Error messages:

* input of a wrong type: `filter 'F' cannot take T` (T is the type name of the input);
* argument number N (1-based) of a wrong type or value: `bad argument N for filter 'F'`.

**R51. Filter names and argument counts.** A filter name not in R52-R59 is the syntax error
`unknown filter 'F'`. A known filter with a number of arguments outside its entry is the syntax error
`wrong number of arguments for filter 'F'`. Argument counts: `slice`, `pad`: 1 or 2; `group`: 0 or 1;
`truncate`, `split`, `count`, `join`, `get`, `fixed`, `plus`: 1; `replace`, `plural`: 2; all others: 0.

In the tables below, "int" never includes bool, and an input type not listed is `filter 'F' cannot take T`.

**R52. Text filters.**

| filter         | input   | arguments | result                                                            |
|----------------|---------|-----------|-------------------------------------------------------------------|
| `upper`        | string  |           | ASCII `a`-`z` become `A`-`Z`; every other character unchanged (`é` stays `é`) |
| `lower`        | string  |           | ASCII `A`-`Z` become `a`-`z`; every other character unchanged     |
| `trim`         | string  |           | removes whitespace (space, `\t`, `\n`, `\r`) at both ends         |

**R53. Length and order.**

| filter         | input          | arguments           | result                                                 |
|----------------|----------------|---------------------|--------------------------------------------------------|
| `length`       | string, list, map |                  | number of characters / items / keys (an int)           |
| `reverse`      | string, list   |                     | the characters / items in reverse order                |
| `slice A [B]`  | string, list   | A, B: int           | the characters / items from position A up to but not including position B (B defaults to the length); a negative A or B counts from the end (the length is added to it); then both are limited to the range 0..length; nothing when A >= B (for `"abcdef"`, A = 1 and B = -1 give `bcde`) |

**R54. Padding and cutting.**

| filter         | input          | arguments           | result                                                 |
|----------------|----------------|---------------------|--------------------------------------------------------|
| `pad W [C]`    | string, int    | W: int; C: a string of exactly one character, default `" "` | an int input is first formatted in decimal; if W >= 0, C is added on the LEFT until the length is W; if W < 0, C is added on the RIGHT until the length is -W; a longer value is never cut |
| `truncate N`   | string         | N: int >= 0         | if the string is longer than N characters: its first N characters followed by `...` (the three dots are NOT counted in N); otherwise unchanged |

**R55. Searching.**

| filter          | input        | arguments                    | result                                          |
|-----------------|--------------|------------------------------|-------------------------------------------------|
| `split S`       | string       | S: non-empty string          | the list of the parts between the occurrences of S (`"a,,b"` split by `","` gives `["a", "", "b"]`; splitting `""` gives `[""]`) |
| `replace O N`   | string       | O: non-empty string; N: string | every occurrence of O replaced by N, scanning from left to right without overlaps |
| `count X`       | string, list | for a string input X must be a non-empty string; for a list input X is any value | string: the number of non-overlapping occurrences of X, scanning from the left (`"aa"` occurs 2 times in `"aaaa"`); list: the number of items equal to X by R29 |

**R56. List filters.**

| filter         | input   | arguments    | result                                                                   |
|----------------|---------|--------------|--------------------------------------------------------------------------|
| `join S`       | list    | S: string    | items formatted by R25 (not escaped), joined with S                      |
| `first`        | list    |              | the first item, or none if the list is empty                             |
| `sort`         | list    |              | a new list sorted ascending; the items must be all ints or all strings (an empty list is fine); any other list is the error `filter 'sort' cannot sort these items` |
| `min`, `max`   | list    |              | the smallest / largest item; none for an empty list; the items must be all ints or all strings, else `filter 'min' cannot compare these items` (or `'max'`) |

**R57. Map and lookup filters.**

| filter         | input      | arguments | result                                                                  |
|----------------|------------|-----------|-------------------------------------------------------------------------|
| `keys`         | map        |           | the list of keys in ascending code point order                          |
| `items`        | map        |           | a list of two-item lists `[key, value]`, in ascending key order         |
| `get K`        | map, list  | map: K is a string; list: K is an int | map: the value of key K, none if there is no such key; list: the item at index K (a negative K counts from the end), none if out of range |

**R58. Number filters.**

| filter          | input              | arguments            | result                                       |
|-----------------|--------------------|----------------------|----------------------------------------------|
| `group [S]`     | int                | S: string, default `","` | decimal digits grouped by three from the right, separated by S; a minus sign stays in front: `-1234567` gives `-1,234,567` |
| `fixed N`       | int                | N: int from 0 to 6   | the value divided by 10 to the power N, written in decimal with exactly N digits after a `.` (no `.` when N is 0), with at least one digit before the `.` and a `-` in front of negative values: with N = 2, `1234` gives `12.34` and `-5` gives `-0.05` |
| `abs`           | int                |                      | the absolute value                           |
| `plus X`        | int, string, list  | X: same type as the input | int: the sum; string or list: the input followed by X |
| `plural O M`    | int                | O, M: strings        | O when the value is exactly 1, else M (also for 0 and -1) |

**R59. safe.** `safe` takes any input and returns it unchanged (R27).

## 11. Errors

**R60. Block structure.** Tags are matched in order: `elif` is valid only inside an `if` before its `else`;
`else` is valid once inside an `if` or a `for`; `end` closes the innermost open block; `break`/`continue`
follow R39; `macro` follows R43. The messages are `unexpected 'elif'`, `unexpected 'else'`,
`unexpected 'end'`, `unexpected 'break'`, `unexpected 'continue'`, `macro must be at the top level` and
`macro 'NAME' is already defined`, for the first such tag. If blocks are still open at the end of the
template, the innermost one is reported at its opening tag: `unclosed 'if'`, `unclosed 'for'`,
`unclosed 'capture'` or `unclosed 'macro'`. Only then are calls checked (R49).

    {: for x in xs :}{: elif y :}{: end :}          ERROR  1:18: unexpected 'elif'
    {: if a :}{: for x in xs :}{: end :}            ERROR  1:1: unclosed 'if'

**R61. Positions.** Lines and columns are 1-based. Lines are separated by `\n` only; a column counts
characters (a tab is one character). Positions always refer to the template source as written (before any
whitespace control). The position of an error is the position of the `{` that starts:

* the unterminated construct (output tag, statement tag, comment, or the verbatim opening tag);
* the tag with a syntax error;
* the tag a structure problem is about (for an unclosed block, its opening tag);
* the tag being evaluated for a rendering error: the output tag, the `set` tag, the `if` or `elif` tag whose
  condition failed, the `for` tag whose iterable failed, or the `call` tag (argument and nesting errors).
  An error inside a macro body is reported at the tag inside the body.

**R62. Which error is reported.** The template is checked in four phases and the first phase that finds an
error reports exactly one error; rendering starts only when phases 1-3 found nothing.

1. Lexing (left to right, stops at the first problem): `unterminated output tag`,
   `unterminated statement tag`, `unterminated comment`, `unterminated verbatim block`.
2. Tag syntax: every tag is checked; the first tag in template order that has a problem is reported. Within
   one tag: first the tokens (R14-R15: `unexpected character 'X'`, `unterminated string`, `bad escape '\X'`),
   then the grammar (R20: `empty tag`, `unknown statement 'T'`, `unexpected token 'T'`,
   `unexpected end of tag`), then the names, of which the first by token position is reported:
   `unknown filter 'F'`, `wrong number of arguments for filter 'F'`, `'safe' must be the last filter`,
   `unknown loop variable '#name'`, `duplicate parameter 'p'`, `parameter 'p' needs a default` (for one
   filter: unknown name first, then the argument count, then the `safe` position).
3. Block structure (R60, then R49).
4. Rendering, in output order; the first error stops it (R63).

So a lexing problem at the very end of the template is reported instead of a syntax problem at its start, a
broken tag anywhere is reported instead of a stray `{: end :}` before it, an unknown macro is reported only
when the block structure is fine, and no rendering error is ever reported for a template that has a syntax
or structure problem.

    {: end :}{= x | upper | =}                      ERROR  1:10: unexpected end of tag
    {= x | nope =}{= y                              ERROR  1:15: unterminated output tag

**R63. Rendering errors.** The rendering messages are: `undefined variable 'P'`, `cannot output a map`,
`cannot compare A and B`, `cannot test membership of T in string`, `cannot test membership in T`,
`cannot iterate over T`, `two loop variables need a map`, `filter 'F' cannot take T`,
`bad argument N for filter 'F'`, `filter 'sort' cannot sort these items`,
`filter 'min' cannot compare these items` (and `'max'`), `macro 'NAME' takes at most K arguments, got M`,
`macro 'NAME' needs at least K arguments, got M`, `macro calls nested too deeply`.

## 12. Command line

**R64. CLI.** `python3 -m stencil TEMPLATE [CONTEXT]` renders the file TEMPLATE (read as UTF-8 bytes, no
newline translation) with the JSON object in the file CONTEXT (an empty context when CONTEXT is not given).
Checks happen in this order:

* not exactly one or two arguments: prints `usage: python3 -m stencil TEMPLATE [CONTEXT]` and a newline to
  stderr, exit status 2;
* TEMPLATE cannot be read: `stencil: cannot read TEMPLATE` (the path as given) and a newline on stderr,
  exit status 2;
* CONTEXT cannot be read, is not valid JSON, or is not a JSON object: `stencil: bad context` and a newline on
  stderr, exit status 2;
* a TemplateError: `stencil: ` followed by `str(err)` and a newline on stderr, nothing on stdout, exit
  status 1;
* success: the rendered text exactly (UTF-8, no newline added) on stdout, nothing on stderr, exit status 0.
