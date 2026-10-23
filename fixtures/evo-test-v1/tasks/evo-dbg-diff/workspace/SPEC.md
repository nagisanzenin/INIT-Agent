# ldiff: line diff and patch

`ldiff` compares two text files line by line and writes their differences as a unified diff, applies a unified
diff to a file, and summarizes a unified diff. Everything in this document is normative: the program must behave
exactly as described.

    python3 run.py diff  [-q] [-N] [-i] [-b | -w] [-u | -U N] [-L LABEL]... OLD NEW
    python3 run.py patch [-F N] [-R] [-p N] [-o OUT] [-r REJ] [-b] [-s] [--dry-run] [FILE] PATCHFILE
    python3 run.py stat  [-R] PATCHFILE

Indexes are 0-based and line numbers are 1-based throughout.

## 1. Lines

1.1 Files are UTF-8 text. A file that is not valid UTF-8 is an error (section 10).

1.2 A file's content is split at each newline character (`\n`). A line's text does not include the newline. If the
content is not empty and does not end with a newline, its last line *lacks a newline*. An empty file has no lines.
Every other character is ordinary text, including carriage return (`\r`), form feed (`\f`) and vertical tab
(`\v`): a file with Windows line ends (`\r\n`) has lines whose text ends with `\r`.

1.3 Two lines are *equal* when their texts are equal and either both have a newline or both lack one. So a file
whose last line lacks a newline differs from the same file with the newline added. (The diff options `-i`, `-b`
and `-w` of 5.2 compare transformed texts instead; the newline rule stays.)

1.4 Lines are written back (the patched file of section 8) as each line's text followed by a newline, except that
the last line of the file gets no newline when it lacks one. A line that lacks a newline but is not the last line
of the file is written with a newline.

## 2. Alignment

The diff command compares old lines a[0..n) with new lines b[0..m). The edit script is computed exactly as follows,
so it is unique.

2.1 Common prefix: the largest p such that a[i] equals b[i] for every i < p. These lines are kept.

2.2 Common suffix: then the largest q <= min(n, m) - p such that a[n-1-k] equals b[m-1-k] for every k < q. These
lines are kept.

2.3 The middle parts A = a[p..n-q) and B = b[p..m-q) are aligned by a longest common subsequence. Let L(i, j) be
the length of a longest common subsequence of A[i..] and B[j..] (0 when either part is empty). Start with
i = j = 0 and repeat while i < len(A) and j < len(B):

- if A[i] equals B[j]: keep the line; i += 1 and j += 1;
- otherwise, if L(i+1, j) >= L(i, j+1): delete A[i]; i += 1;
- otherwise: insert B[j]; j += 1.

Then delete the rest of A and insert the rest of B.

2.4 A *change block* is a maximal run of deletions and insertions with no kept line in between. It removes the old
lines [os, oe) and adds the new lines [ns, ne); one of the two ranges may be empty. A diff always lists all the
deleted lines of a block first (in old order), then all its inserted lines (in new order).

## 3. Hunks

3.1 The context size N is 3 unless `-U` gives another value (N >= 0).

3.2 The change blocks are grouped in order. A block joins the group of the block before it when the number of kept
lines between the two blocks (os of this block minus oe of the previous block) is at most 2N; otherwise it starts
a new group.

3.3 Each group forms one hunk. The hunk starts min(N, os) lines before the old start os of its first block and
ends min(N, n - oe) lines after the old end oe of its last block. Its lines, in order: the leading context lines,
then for each block its deleted lines followed by its inserted lines, with the kept lines between two blocks of the
group as context lines, then the trailing context lines. Context lines are kept lines, equal in both files.

## 4. Unified diff format

This format is the output of the diff command, the input of the patch and stat commands and the format of reject
files.

4.1 A diff starts with the two header lines `--- OLDLABEL` and `+++ NEWLABEL`, followed by its hunks.

4.2 Each hunk starts with the line `@@ -OLDRANGE +NEWRANGE @@`. For a side on which the hunk covers c lines, the
first of which is line number s, the range is:

- `s` when c is 1 (the count is left out);
- `s,c` when c is 2 or more;
- `t,0` when c is 0, where t is the number of lines of that side that come before the place where the hunk
  applies (the line after which the other side's lines go, or 0 at the top of the file).

Example: inserting two lines after old line 5, where they become new lines 6 and 7, with no context lines:
`@@ -5,0 +6,2 @@`. Deleting old lines 1 and 2 of a file with no context lines: `@@ -1,2 +0,0 @@`.

4.3 Then the hunk's lines: a context line is a space followed by its text, a deleted line is `-` followed by its
text, an inserted line is `+` followed by its text.

4.4 A line that lacks a newline (section 1.2) is followed by the marker line `\ No newline at end of file`. For a
context line this means the line lacks a newline in both files.

4.5 Every line of a diff (header lines, hunk headers, hunk lines, markers) ends with a newline.

## 5. The diff command

    python3 run.py diff [options] OLD NEW

5.1 OLD and NEW are file paths; `-` means standard input (it may be given for at most one of them).

5.2 Options:

- `-U N`, `-UN` or `--unified=N`: the context size N (section 3.1); `-u` is the same as `-U 3`;
- `-L LABEL`, `--label LABEL` or `--label=LABEL`: the first label given replaces OLD in the `---` line, the second
  replaces NEW in the `+++` line; more than two labels is an error;
- `-q` or `--brief`: when the files differ, print only `Files OLD and NEW differ` (OLD and NEW as given, labels
  play no part) instead of the diff; the exit status is the same as without `-q`;
- `-N` or `--new-file`: a file (OLD or NEW, not `-`) that does not exist is compared as an empty file; when
  neither exists it is still an error;
- `-i` or `--ignore-case`: texts are compared after converting them to lower case;
- `-b` or `--ignore-space-change`: texts are compared after replacing every run of spaces and tabs by one space
  and then removing a trailing space;
- `-w` or `--ignore-all-space`: texts are compared after removing every space and tab (when both `-b` and `-w`
  are given, `-w` applies).

With `-i`, `-b` or `-w`, two lines are equal when their transformed texts are equal and their newline status
(1.3) is the same; everything in sections 2 and 3 uses that equality, and context lines are written as they are
in OLD. `-i` combines with `-b` or `-w` (the case is converted after the spaces are handled).

Without labels, the header lines show OLD and NEW exactly as they were given on the command line.

5.3 Options and operands may come in any order; `--` ends the options (every argument after it is an operand).
A later `-U` (or `-u`) overrides an earlier one.

5.4 When there is no change block, the command prints nothing and exits with status 0. Otherwise it prints the
unified diff of sections 3 and 4 on standard output and exits with status 1.

## 6. Reading a patch

6.1 The patch text is split into lines as in section 1.2; here every line is just text.

6.2 The lines before the file header are ignored. The file header is the first line that starts with `--- `
(three dashes and a space); the next line must start with `+++ `. A label is the rest of its header line after
those four characters, up to (and not including) the first TAB, if there is one.

6.3 One or more hunks follow. A hunk header is a line of the form `@@ -R +R @@`, after which anything may follow
on the same line. R is `s` or `s,c`, both decimal numbers; a count that is left out is 1. A range whose count is
1 or more must have s >= 1.

6.4 The hunk body is read until the hunk holds as many old-side lines (context and `-` lines) and as many new-side
lines (context and `+` lines) as its header says. A body line that starts with a space is a context line, `-` a
deleted line, `+` an inserted line; the rest of the line is the text. An empty line is a context line with empty
text. A line that starts with `\` is a marker: the line before it in the same hunk lacks a newline. A marker may
also come right after the last line of a hunk.

6.5 After a hunk (and its marker, if any), a line that starts with `@@` starts the next hunk. Any other line ends
the *file section* (the file header and its hunks). The lines after it are skipped up to the next line that starts
with `--- `, which starts the next file section, read like the first one (6.2 to 6.4, and 6.7 applies to it); when
there is no such line, the patch ends. A patch has one or more file sections, in order.

6.6 The position of a range is s - 1 when its count is 1 or more, and t when it is `t,0` (so a hunk with an empty
old side applies after the first t lines). The old position of a hunk is the position of its old range.

6.7 A patch is malformed (section 10) when: it has no file header; the `---` line is not followed by a `+++` line,
or the `+++` line not by a hunk header; a hunk header does not have the form of 6.3, or a range breaks the rule of
6.3; the text ends inside a hunk body; a body line starts with any other character; a hunk would get more old-side
or more new-side lines than its header says; a marker has no line before it in its hunk, or follows a line that
already has one; a hunk has neither `-` nor `+` lines; a hunk's old position is smaller than the old position plus
the old count of the hunk before it (hunks must be in order and must not overlap).

## 7. Applying a patch

7.1 A hunk's old side is the list of its context and `-` lines, in order; its new side is the list of its context
and `+` lines, in order. Its leading context is the number of context lines before its first `-` or `+` line;
its trailing context is the number of context lines after its last `-` or `+` line.

7.2 The hunks of a file section are applied one after the other, in patch order, to a working copy of the lines
of its file. Three values are carried from hunk to hunk, all 0 at the start of every file section:

- shift: the sum, over every hunk applied so far, of its new-side line count minus its old-side line count;
- offset: the offset (7.5) of the most recently applied hunk;
- floor: the index just after the new-side lines of the most recently applied hunk, in the working copy.

A rejected hunk changes none of the three.

7.3 For each hunk: base = its old position + shift, and expected = base + offset.

7.4 Matching. At fuzz level f, a hunk ignores the first min(f, leading context) lines and the last
min(f, trailing context) lines of its old side. It matches at position p when every old-side line that is not
ignored is equal (section 1.3) to the working-copy line it lies on: old-side line i lies on working line p + i.
Only positions with floor <= p and p + (old-side line count) <= (working-copy line count) are considered, so the
ignored lines must lie inside the file too.

7.5 Search. The fuzz levels 0, 1, ..., F are tried in that order (F is 2 unless `-F` gives another value). At each
level the positions are tried in order of their distance from expected, nearest first; at equal distance d the
later position (expected + d) comes before the earlier one (expected - d). The first match found is used, so a
match without fuzz anywhere in the file is preferred to a match with fuzz at the expected position. The hunk's
offset is p - base. When no level gives a match, the hunk is rejected.

7.6 A match at position p with fuzz f, ignoring lead lines at the start and trail lines at the end, replaces the
working lines p + lead up to (not including) p + (old-side line count) - trail with the new side without its first
lead and last trail lines. The ignored lines keep the text they have in the file.

7.7 Messages, one line per hunk that needs one, in hunk order (k counts the hunks of the patch from 1):

- a hunk applied with offset 0 and fuzz 0 gives no message;
- any other applied hunk gives `Hunk #k succeeded at L`, then ` with fuzz f` when f > 0, then ` (offset d lines)`
  when its offset d is not 0, then `.`; L is p + 1. It is `line` instead of `lines` when d is 1 or -1. Examples:
  `Hunk #2 succeeded at 14 (offset 3 lines).`, `Hunk #1 succeeded at 7 with fuzz 1.`,
  `Hunk #3 succeeded at 40 with fuzz 2 (offset -1 line).`;
- a rejected hunk gives `Hunk #k FAILED at L.` where L is expected + 1.

Everything in this section is done per file section: the messages, the rejected hunks and the counts of 8.4 and
8.5 belong to one file section and never carry over to the next one.

## 8. The patch command

    python3 run.py patch [options] [FILE] PATCHFILE

8.1 PATCHFILE `-` reads the patch from standard input. FILE must be an existing file (it may be empty); it cannot
be `-`; when FILE is given, the patch must have exactly one file section. When FILE is left out, every file
section is applied, in order, to the file taken from that section: its `+++` label, or its `---` label when the `+++`
label is `/dev/null` (labels as exchanged by `-R`), without its first N path components with `-p N` (the
components are the parts between `/` characters; `-p1` turns `b/src/a.txt` into `src/a.txt`). It is an error when
that label is empty or `/dev/null` or has N or fewer components, when two file sections name the same file, and when
`-o` or `-r` is given with a patch of more than one file section. Every file is read before any section is applied.

8.2 Options (in any order with the operands; `--` ends the options):

- `-F N`, `-FN` or `--fuzz=N`: the highest fuzz level F (section 7.5);
- `-R` or `--reverse`: apply the patch in reverse: in every hunk the old and new ranges are exchanged and the `-`
  and `+` lines are exchanged (the order of the lines does not change), and the two labels are exchanged;
- `-p N`, `-pN` or `--strip=N`: the number of path components removed from the label (8.1; default 0);
- `-o OUT`, `-oOUT` or `--output=OUT`: write the patched lines to the file OUT instead of FILE (FILE is not
  changed);
- `-r REJ`, `-rREJ` or `--reject-file=REJ`: the name of the reject file (default: the written file's name
  followed by `.rej`, so FILE.rej, or OUT.rej with `-o`);
- `-b` or `--backup`: before the patched lines are written, if the file about to be written exists, its current
  content is saved, byte for byte, in that file's name followed by `.orig`;
- `-s`, `--silent` or `--quiet`: print nothing on standard output (files and exit status are unchanged);
- `--dry-run`: do everything except writing files.

8.3 The patch is read and checked completely before anything else; when it is malformed, or a file cannot be read,
nothing is printed on standard output and no file is written (section 10).

8.4 Standard output, for each file section in order: first `patching file FILE` (with `--dry-run`: `checking file FILE`), FILE as given or as
taken from the label; with `-o OUT` (OUT different from FILE) the line is `patching file OUT (read from FILE)`.
Then the hunk messages (7.7). Then, when R of the M hunks were rejected,
`R out of M hunks FAILED -- saving rejects to file REJ` with REJ the reject file name (`hunk` instead of `hunks`
when M is 1; with `--dry-run` the line ends right after `FAILED`).

8.5 Files: the patched lines (section 1.4) are written to FILE (or OUT), also when some hunks were rejected.
When at least one hunk was rejected, the reject file is written as well: the header lines `--- OLDLABEL` and
`+++ NEWLABEL` (the labels of the patch, exchanged by `-R`), then every rejected hunk, in patch order, in the
format of section 4: its ranges are written from its positions and counts (exchanged by `-R`) in the forms of 4.2,
whatever form the patch used (for example `-3,1` is written `-3`), and its lines keep their markers. With
`--dry-run` no file is written.

8.6 Exit status: 0 when every hunk of every file section was applied, 1 when at least one hunk was rejected.

## 9. The stat command

    python3 run.py stat [-R] PATCHFILE

9.1 PATCHFILE (`-` for standard input) is read as in section 6; `-R` (or `--reverse`) exchanges the sides as in
8.2.

9.2 Standard output, for each file section in order: the line `OLDLABEL -> NEWLABEL`; then one line per hunk, in
order: its hunk header written from its positions and counts in the forms of 4.2 (like a reject file), a space,
`-D +I` where D is its number of `-` lines and I its number of `+` lines. After the last section:
`H hunks, D deletions, I insertions` with the totals over all sections (`hunk`, `deletion`, `insertion` when the
number is 1), preceded by `F files, ` when the patch has more than one file section. Example hunk line:
`@@ -3,2 +3,4 @@ -1 +3`. Exit status 0.

## 10. Errors

10.1 These are errors: an unknown command or option; an option without its value; a value of `-U`, `-F` or `-p`
that is not a decimal integer >= 0; the wrong number of operands; `-` given for both OLD and NEW, or as FILE; more
than two labels; a file that does not exist (except with `-N`, 5.2), cannot be read or is not valid UTF-8; a
malformed patch (6.7); a FILE that cannot be taken from the patch, and the other errors of 8.1.

10.2 On an error the program prints a one-line message that starts with `run.py: ` on standard error, prints
nothing on standard output, writes no file and exits with status 2.
