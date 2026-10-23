# mkmanifest.sh

    sh mkmanifest.sh [-o OUTFILE] [-n NAME] [-x PATTERN]... [-v] DIR...

Writes a manifest of the regular files under each DIR (recursively):

- The first line is `# manifest: NAME`. NAME defaults to the base name of the first DIR.
- Then one line per regular file: its path as `DIR/...` (DIR exactly as given on the command line, no
  `./` added), sorted by byte value (as `LC_ALL=C sort`). Directories and symbolic links are not listed.
- `-x PATTERN` (repeatable) leaves out files whose base name matches the shell glob PATTERN; every -x counts.
- `-o OUTFILE` writes the manifest to OUTFILE instead of standard output (`-o -` is standard output).
- `-v` also prints `N files` (N = the number of file lines) to standard error.
- An unknown option or a missing DIR prints a line starting with `usage:` to standard error, writes nothing
  to standard output, and exits with status 2. A DIR that is not a directory is an error too (status 2, message on stderr).
- Options come before the DIRs; `--` ends the options. DIRs and file names may contain spaces.
- Exit status 0 on success.
