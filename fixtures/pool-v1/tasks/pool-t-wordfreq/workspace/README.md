# wordfreq

    bin/wordfreq [-n N] [FILE...]

Prints the N most frequent words (default 10) of the named files taken together, or of standard input when
no FILE is given.

- A word is a maximal run of ASCII letters (a-z, A-Z); everything else separates words. Words are counted
  case-insensitively and printed in lower case. An apostrophe between two letters joins them (`don't` is one
  word, `'quoted'` is the word `quoted`).
- Output: one line per word, `COUNT<TAB>WORD`, most frequent first; equal counts are ordered by word
  (byte order, a-z).
- Fewer than N distinct words: print them all. `-n 0` prints nothing.
- A FILE that cannot be read: print `wordfreq: cannot read FILE` to standard error, go on with the other files,
  and exit with status 1 at the end (output is still printed). Otherwise exit status 0.
- `-n` with a value that is not a non-negative integer: print a line starting with `usage:` to standard
  error and exit with status 2 (no output).
