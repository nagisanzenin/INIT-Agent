# sheet: specification

`sheet` is a small spreadsheet engine. It reads a workbook from a CSV file, evaluates the formulas in it,
and either prints the evaluated sheet or runs a script of edits against it, recalculating incrementally.
This document is the complete definition of its behaviour.

## 1. Running

    python3 run.py BOOK.csv [EDITS]

- `BOOK.csv` holds the initial content of the sheet (section 2). It is loaded and every formula is calculated
  (section 8).
- Without `EDITS`, the program prints the whole evaluated sheet as CSV (section 4) to stdout and exits 0.
- With `EDITS`, the program runs the commands of that file in order (section 9). Only the commands print;
  nothing else is written to stdout. Exit status 0.
- Errors: the program writes one line `run.py: MESSAGE` to stderr and exits with status 2. Whatever the
  earlier commands printed stays on stdout. The messages are:
  - wrong number of arguments: `run.py: usage: run.py BOOK.csv [EDITS]`
  - a file that cannot be read or is not valid UTF-8: `run.py: cannot read PATH` (PATH as given)
  - a quoted CSV field that is never closed: `run.py: PATH: unterminated quoted field`
  - an EDITS line that is not a valid command: `run.py: line N: bad command` (N counts every line of the
    file from 1, including blank and comment lines). The commands before it have run.

## 2. The book file and cell content

**CSV.** The file is UTF-8 text. Records end with `\n`; a `\r\n` pair also ends a record. Fields are
separated by `,`. A field that begins with `"` is quoted: it ends at the next `"` that is not doubled, a doubled
`""` inside it stands for one `"`, and it may contain commas, `\r` and `\n`. A field that does not begin with
`"` is taken literally up to the next comma or end of record. The last record may or may not end with a
newline; an empty line is a record with a single empty field. Record *n* is row *n* of the sheet, and field
*k* of a record is column *k* (column 1 is A).

**Content.** Quoting only transports text: after unquoting, every field is cell content text, typed as
follows (the same rules apply to the content given by the `set` command, section 9):

| content text | the cell holds |
|---|---|
| empty | nothing: the cell is blank |
| starts with `=` | a formula: the text after `=` (section 5) |
| starts with `'` | the string after the apostrophe (`'123` is the string `123`, `'` alone the empty string) |
| number syntax (below) | that number |
| `TRUE` or `FALSE`, in any letter case | that logical value |
| anything else | that string, exactly as written (spaces included) |

Number syntax: an optional `+` or `-`, then digits with an optional `.` and fraction digits (`12`, `4.`,
`3.25`) or a `.` followed by digits (`.5`), then an optional exponent: `e` or `E`, an optional sign and digits
(`1e3`, `2.5E-2`). Nothing else is allowed, not even surrounding spaces: ` 12` is a string.

## 3. Values and their display

Every cell has a value of one of five types: **number**, **string**, **logical** (`TRUE`/`FALSE`), **blank**
(no content), **error**. The error values are `#DIV/0!`, `#VALUE!`, `#REF!`, `#NAME?`, `#CYCLE!` and
`#PARSE!`.

The *text form* of a value is used to display it and wherever a value is converted to text (`&`, `LEN`,
`UPPER`, `CONCAT`):

- a number that is a whole number with absolute value below 10^15 is written as an integer: digits with a
  leading `-` when negative, no decimal point, no exponent (`-0` is written `0`);
- any other number is written as C's `printf("%.10g")` writes it: at most 10 significant digits, trailing
  zeros of the fraction removed, exponent form when the exponent is below -4 or at least 10. Examples:
  `1/3` gives `0.3333333333`, `2/3` gives `0.6666666667`, `0.1+0.2` gives `0.3`, `10^15` gives `1e+15`,
  `1.5e-7` gives `1.5e-07`, `123456789012.5` gives `1.23456789e+11`;
- a string is itself; a logical value is `TRUE` or `FALSE`; blank is the empty text;
- an error is its code, e.g. `#DIV/0!`.

## 4. Output

The **used range** is rows 1..R and columns 1..C, where R is the largest row and C the largest column of any
cell that holds content (a formula cell holds content whatever its value). If no cell holds content, the used
range is empty and printing it prints nothing.

The sheet (or any range) is printed one row per line, each line ending with `\n`, holding exactly one field per
column of the range, separated by commas. A field is the text form of the cell's value. It is written between
double quotes, with every `"` inside doubled, when it contains a comma, a `"`, a `\r` or a `\n`, or when it
begins or ends with a space; otherwise it is written as is (an empty field is written as nothing).

## 5. Formulas

### 5.1 Tokens

Spaces and tabs between tokens are ignored. The tokens are:

- **number**: digits with an optional `.` and fraction digits, or `.` and digits, then an optional exponent
  (`3`, `3.25`, `.5`, `1e3`, `1.5E-3`). A formula number has no sign; `-` is an operator.
- **string**: text between double quotes; a doubled `""` inside stands for one `"`. `"a""b"` is `a"b`.
- **error literal**: one of the six error codes of section 3, in any letter case.
- **word**: a letter, `_` or `$`, followed by letters, digits, `_` or `$`. A word is
  - a **function name** when it is followed directly (no space) by `(` and contains no `$`;
  - otherwise a **reference** when it has the form: optional `$`, 1 to 3 letters (the column), optional `$`,
    a row number of 1 to 7 digits not starting with `0`. Letters may be in any case: `b7` is `B7`;
  - otherwise `TRUE` or `FALSE` (any case) is a logical literal;
  - otherwise, a word containing `$` is a syntax error, and any other word is a **name**.
- **operators**: `+ - * / ^ & = <> < <= > >=`, and `(`, `)`, `,`, `:`.

Any other character is a syntax error.

**Columns** are named in bijective base 26: `A`..`Z` are columns 1..26, then `AA` (27), `AB` (28), ..., `AZ`
(52), `BA` (53), ..., `ZZ` (702), `AAA` (703), ..., `ZZZ` (18278). The `$` anchors only matter when a formula
is copied (section 9.2); `$B$7`, `$B7`, `B$7` and `B7` all name the same cell.

### 5.2 Grammar and precedence

A formula is one expression. From the tightest binding to the loosest:

1. **primary**: number, string, logical or error literal; reference; range `REF:REF` (two references joined by
   a colon, for example `A1:C3` or `$B$2:B9`; the corners may be given in any order); function call
   `NAME(arg, ...)` with zero or more arguments that are expressions; parenthesized expression; name.
2. `^` (power), **right**-associative: `2^3^2` is `2^(3^2)` = 512.
3. prefix `-` and `+`. A prefix sign applies to the whole power expression that follows it: `-2^2` is
   `-(2^2)` = -4, and `-A1^2` is never positive. Signs can repeat (`--3` is 3), and the exponent of `^` may start
   with a sign, which then applies to the power expression that follows it: `2^-1` = 0.5, `2^-2^2` = `2^(-(2^2))`
   = 0.0625.
4. `*` and `/`, left-associative: `8/4/2` = 1.
5. `+` and `-`, left-associative: `1-2-3` = -4.
6. `&` (text concatenation), left-associative.
7. comparisons `=`, `<>`, `<`, `<=`, `>`, `>=`, left-associative: `1<2=TRUE` is `(1<2)=TRUE`.

More examples: `-2^2*3` is `(-(2^2))*3` = -12; `2*-3^2` is `2*(-(3^2))` = -18; `10-2^2` = 6; `(-2)^2` = 4;
`2^3*2` = 16; `1+2&3` is `(1+2)&3`, the string `33`.

A formula that does not match this grammar (empty formula, unbalanced parentheses, two operands in a row,
an empty argument such as `SUM(1,,2)`, an unterminated string, a stray character, ...) has the value `#PARSE!`.
Function names are case-insensitive.

## 6. Evaluation

### 6.1 References, ranges, names

- A reference evaluates to the value of the cell it names; a cell without content is **blank**.
- A range is only meaningful as a function argument where section 7 says so. A range anywhere else (for
  example `=A1:A3`, `=A1:A3+1`, or as the result of `IF`) evaluates to `#VALUE!`.
- A name (a word that is neither a reference, a function nor a logical literal) evaluates to `#NAME?`. A call
  of a function not listed in section 7 evaluates to `#NAME?`, without evaluating its arguments.
- The value of a formula cell is the value of its expression, except that a **blank** result becomes the
  number 0 (`=A9` with A9 empty shows `0`). Inside an expression a blank stays blank (`=A9&"x"` is `x`).

### 6.2 Conversions

- **to number**: a number is itself; blank is 0; `TRUE` is 1 and `FALSE` 0; a string is converted with the
  number syntax of section 2 after removing leading and trailing spaces (`" 3 "` is 3), and any other string,
  including the empty string, gives `#VALUE!`.
- **to text**: the text form of section 3.
- **to logical**: a logical value is itself; a number is `TRUE` unless it is 0; blank is `FALSE`; the strings
  `TRUE` and `FALSE` in any letter case are those values; any other string gives `#VALUE!`.

An error value converts to itself.

### 6.3 Operators

Both operands of a binary operator are evaluated, left first. If the left operand is an error, that is the
result; otherwise if the right operand is an error, that is the result. Only then are the operands converted,
left first, and the first failed conversion gives its error. So `=1/0+"x"` and `="x"+1/0` are both `#DIV/0!`,
and `="x"*"y"` is `#VALUE!`.

- `+ - * /`: both operands to number. Division by zero gives `#DIV/0!`.
- `^`: both operands to number. `0^0` = 1. Zero to a negative power gives `#DIV/0!`. A negative base with a
  non-whole exponent gives `#VALUE!` (`(-8)^(1/3)` is `#VALUE!`, `(-2)^3` is -8).
- Any arithmetic result that is not a finite number (overflow) gives `#VALUE!`.
- prefix `-` and `+`: the operand to number, then negated (`-`) or unchanged (`+`): `=+"3"` is 3.
- `&`: both operands to text, joined: `=1/4&"%"` is `0.25%`, `=TRUE&1` is `TRUE1`.
- comparisons: section 6.4. The result is a logical value.

### 6.4 Comparisons

After the error rule of 6.3:

- If both operands are blank they are equal. If one is blank it is replaced by the empty value of the other's
  type: 0 for a number, the empty string for a string, `FALSE` for a logical value.
- Two numbers compare numerically.
- Two strings compare ignoring letter case: both are lower-cased, then compared character by character by code
  point (`"apple"<"Banana"` is `TRUE`, `"abc"="ABC"` is `TRUE`).
- Two logical values: `FALSE` < `TRUE`.
- Values of different types are never equal, and order by type: every number < every string < every logical
  value. So `=1="1"` is `FALSE`, `=TRUE=1` is `FALSE`, `="z"<TRUE` is `TRUE`, `=99>"a"` is `FALSE`.

### 6.5 Error precedence

When several errors could be the result, the first one met in evaluation order wins: operands as in 6.3,
function arguments left to right, the cells of a range row by row, each row left to right. A function stops at
the first error it meets (section 7.1).

## 7. Functions

A call with a number of arguments outside the range given below evaluates to `#VALUE!` (the arguments are not
evaluated). Arguments are evaluated left to right; `IF` evaluates only the branch it chooses.

### 7.1 Argument values

A **reference argument** is an argument that consists of a single reference or range, possibly inside
parentheses: `SUM(A1)`, `SUM(A1:B3)` and `SUM((A1))` have one, `SUM(A1+0)` and `SUM(-A1)` do not. It supplies
the values of its cells, row by row (*referenced values*). Any other argument supplies its own value (a *direct
value*).

| function | referenced values | direct values |
|---|---|---|
| `SUM`, `AVERAGE`, `MIN`, `MAX` | numbers are used; blanks, strings and logical values are skipped; an error is the result | converted to number (a blank is 0, `TRUE` 1, `"3"` 3, `"x"` gives `#VALUE!`); an error is the result |
| `COUNT` | counts the numbers | counts the values that are numbers |
| `COUNTA` | counts the values that are not blank (errors and empty strings count) | counts the values that are not blank |
| `AND`, `OR` | logical values are used, numbers are used as logical (non-zero is `TRUE`); blanks and strings are skipped; an error is the result | converted to logical (a blank is `FALSE`, `"true"` is `TRUE`, `"x"` gives `#VALUE!`); an error is the result |
| `CONCAT` | the text form of each value, an error is the result | the text form, an error is the result |

`COUNT` and `COUNTA` never produce an error: an error value is not a number, so `COUNT` does not count it, and
`COUNTA` counts it.

### 7.2 The functions

| function | arguments | result |
|---|---|---|
| `SUM(v, ...)` | 1+ | the sum of the numbers (0 when there are none) |
| `AVERAGE(v, ...)` | 1+ | the sum divided by how many numbers were used; no numbers gives `#DIV/0!` |
| `MIN(v, ...)`, `MAX(v, ...)` | 1+ | the smallest / largest number; 0 when there are none |
| `COUNT(v, ...)`, `COUNTA(v, ...)` | 1+ | see 7.1 |
| `IF(cond, then [, else])` | 2-3 | `cond` is converted to logical (an error is the result). If `TRUE`, the value of `then`; otherwise the value of `else`, or `FALSE` when there is no `else`. Only the chosen branch is evaluated: an error (or a `#CYCLE!` reference) in the other branch has no effect |
| `AND(v, ...)`, `OR(v, ...)` | 1+ | `TRUE` when all / at least one of the logical values are `TRUE`; no logical values at all gives `#VALUE!` |
| `NOT(v)` | 1 | `v` converted to logical, negated |
| `ROUND(x [, digits])` | 1-2 | `x` (to number) rounded to `digits` decimal places (to number, the fraction dropped toward zero: 1.9 means 1, -1.9 means -1; default 0). Halves are rounded **away from zero**, judged on the shortest decimal form of `x`: `ROUND(2.5)` = 3, `ROUND(-2.5)` = -3, `ROUND(0.125, 2)` = 0.13, `ROUND(2.675, 2)` = 2.68. Negative `digits` round to tens, hundreds, ...: `ROUND(1250, -2)` = 1300 |
| `ABS(x)` | 1 | absolute value of `x` (to number) |
| `LEN(v)` | 1 | number of characters of the text form of `v` (an error is the result): `LEN(1/3)` = 12 |
| `UPPER(v)` | 1 | the text form of `v` in upper case |
| `CONCAT(v, ...)` | 1+ | the text forms joined, see 7.1 (`CONCAT(A1:B2)` reads A1, B1, A2, B2) |
| `SUMIF(range, criterion [, sum_range])` | 2-3 | see 7.3 |
| `COUNTIF(range, criterion)` | 2 | see 7.3 |

### 7.3 SUMIF and COUNTIF

`range` and `sum_range` must be reference arguments (7.1), otherwise the result is `#VALUE!`; `sum_range` must
have the same number of rows and columns as `range`, otherwise `#VALUE!`. `criterion` is evaluated as a direct
value (an error is the result). Each cell of `range` either meets the criterion or not; a cell holding an error
never meets it.

`COUNTIF` counts the cells that meet the criterion. `SUMIF` adds up, for every cell of `range` that meets the
criterion, the cell at the same offset in `sum_range` (in `range` itself when there is no `sum_range`): numbers
are added, other values skipped, and an error in such a cell is the result (the first in row order). With no
match the sum is 0.

What a criterion matches:

- a **number**: cells holding that number (a string that looks like a number does not match);
- a **logical value**: cells holding that logical value;
- **blank** (a reference to an empty cell) or the empty string: blank cells and cells holding the empty string;
- any other **string**: it may start with an operator `<=`, `>=`, `<>`, `<`, `>` or `=` (checked in that
  order); without one, the operator is `=`. The rest of the string is the operand:
  - empty operand: `=` matches blank cells and empty strings, `<>` matches all other cells (errors excepted),
    and `<`, `<=`, `>`, `>=` match nothing;
  - an operand with the number syntax of section 2 (`">=10"`, `"5"`, `"<>-1.5"`): `=` matches cells holding
    that number; `<>` matches every cell that does not hold that number (blank cells, strings and logical values
    included); the ordering operators match the cells holding a number that compares so;
  - an operand `TRUE` or `FALSE` (any case): the same with logical values (`FALSE` < `TRUE`);
  - any other operand is text: `=` matches strings equal to it ignoring case, `<>` matches every cell that is
    not such a string, and the ordering operators match strings that compare so, ignoring case (as in 6.4).

## 8. Dependencies and recalculation

### 8.1 Dependencies

The **precedents** of a formula cell are all the cells its formula references: every cell named by a
reference and every cell inside a range, whether or not they have content, and whether or not the evaluation
reaches them (both branches of an `IF` count). A formula cell **depends on** each of its precedents, and on
everything they depend on (transitively). Dependencies always reflect the current formulas: after a cell's
formula is replaced, the cells only the old formula referenced are no longer its precedents.

### 8.2 Cycles

A formula cell is **on a cycle** when it depends on itself (`A1` = `=A1+1`, or `A1` = `=B1`, `B1` = `=A1`, or
`A5` = `=SUM(A1:A5)`). A cell on a cycle is not evaluated: its value is `#CYCLE!`. A cell that only depends on a
cycle without being on one is evaluated normally; reading a cycle cell gives the error value `#CYCLE!`, which
then propagates like any error (so `=A1+1` reading a cycle cell is `#CYCLE!`, while `=IF(TRUE, 7, A1)` is 7).
Two paths to the same cell are not a cycle.

### 8.3 Which cells are recalculated

- **Load**: every formula cell of the book.
- **Edit** (`set` or `copy`, section 9): the edited cells that now hold a formula, plus every formula cell that
  depends on an edited cell (after the edit). Nothing else: the other cells keep their values. This holds after
  any number of earlier edits, including edits that added, replaced or removed formulas.
- **Structural edit** (`insert` or `delete`, section 9.5): every formula cell, as at load.

Sheets can be large: thousands of formula cells, dependency chains thousands of cells long (each cell reading
the one above it) and cycles through thousands of cells are calculated by the same rules.

### 8.4 Order

Of the cells to recalculate, those on a cycle get `#CYCLE!` first. The others are evaluated **exactly once
each**, in dependency order: a cell is *ready* when each of its precedents that is to be recalculated has been
evaluated already or is on a cycle. Repeatedly, the ready cell that comes first in row-major order (lowest row,
then lowest column) is evaluated, until all are done. So a cell is never evaluated before a precedent that is
recalculated in the same step.

The `recalc` command (9.4) prints the cells evaluated by the latest load, edit or structural edit in this order;
cells on a cycle are not listed.

## 9. The EDITS script

The script is UTF-8 text, one command per line (a `\r` at the end of a line is dropped). Lines that are empty
or contain only spaces, and lines starting with `#`, are skipped. A command line starts with its keyword in
lower case (`set`, `copy`, `print`, `recalc`, `insert`, `delete`); its parts are separated by single
spaces. References in commands follow section 5.1 (`$` allowed, letters in any case); a command with a missing or malformed part is a bad command (section 1).

### 9.1 `set REF CONTENT`

Sets the content of one cell. `CONTENT` is everything after the space that follows `REF`, kept exactly
(spaces included); it is typed as in section 2 (no CSV unquoting). `set B2` and `set B2 ` (with nothing after
the space) make B2 blank. The cell and its dependents are recalculated (section 8.3).

### 9.2 `copy SRC DEST`

Copies the content of cell `SRC` into every cell of `DEST` (a cell or a range), as one edit. The content of
`SRC` is read before anything is written, so `DEST` may contain `SRC`.

- Content that is not a formula is copied unchanged (a blank `SRC` makes the cells blank).
- In a formula, every reference is moved by the offset from `SRC` to the target cell (rows down and columns
  right are positive): the row moves unless it has a `$` anchor, the column moves unless it has one.
  `=A1+$A1+A$1+$A$1` copied from B2 to D5 becomes `=C4+$A4+C$1+$A$1`. A range moves both corners.
- A moved reference (or range) that would leave the sheet (row below 1 or above 9999999, column before A or
  after ZZZ) is replaced by the error literal `#REF!`: `=A1+B2` copied from B2 to A1 becomes `=#REF!+A1`.

All targets are set first; then the recalculation of 8.3 runs once for all of them.

### 9.3 `print` and `print RANGE`

`print RANGE` prints the values of the cells of `RANGE` (a cell like `B2` or a range like `A1:C3`, corners in
any order) as CSV rows (section 4), whether or not they are inside the used range. `print` alone prints the
used range, exactly as the program prints the sheet when there is no EDITS file.

### 9.4 `recalc`

Prints one line: `recalc:` followed by the names of the cells evaluated by the latest load or edit, in
evaluation order (section 8.4), each preceded by one space. Names use upper-case column letters and no `$`,
e.g. `recalc: B2 C2 AA7`. When no cell was evaluated the line is just `recalc:`.

### 9.5 `insert row N`, `delete row N`, `insert col X`, `delete col X`

`N` is a row number (1 to 7 digits, not starting with `0`), `X` a column name (1 to 3 letters, any case).

- `insert row N` inserts an empty row before row N: every cell in row N or below moves down one row.
- `delete row N` deletes row N: its cells are removed and every cell below it moves up one row.
- `insert col X` and `delete col X` do the same with columns (cells move right / left).

Every formula in the sheet (also in cells that move) is rewritten so that its references keep naming the same
cells. `$` anchors do not matter here: anchored and relative parts move alike. For `insert row N`:

- a cell reference to row N or below moves down one row; a range moves each corner by the same rule (so a range
  that starts above row N and ends in row N or below grows by one row, and one that starts in row N or below
  moves down).

For `delete row N`:

- a cell reference to row N becomes `#REF!`; a reference to a row below N moves up one row;
- a range covering rows T to B (whatever the order its corners are written in): when T = B = N it becomes
  `#REF!`; otherwise its first row becomes T-1 if T > N (else stays T) and its last row becomes B-1 if B >= N
  (else stays B). So a range loses the deleted row and keeps the rest: with row 3 deleted, `A3:A6` becomes
  `A3:A5`, `A1:A3` becomes `A1:A2`, `A4:A9` becomes `A3:A8`, and `A3:A3` becomes `#REF!`.

Columns follow the same rules with columns in place of rows. A reference that would move past row 9999999 or
column ZZZ becomes `#REF!`. Cells that hold no formula keep their content. Then every formula cell is
recalculated (8.3), and `recalc` lists them.
