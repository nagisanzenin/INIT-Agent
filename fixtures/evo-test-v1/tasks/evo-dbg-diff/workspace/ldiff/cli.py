"""The command line (SPEC.md sections 5, 8 and 9): the `diff` and `patch` commands and their options."""
import os
import re
import sys

from .applier import DEFAULT_FUZZ, apply_hunks, reject_text, summary
from .hunks import DEFAULT_CONTEXT, DELETED, INSERTED, make_hunks
from .lcs import comparison
from .parser import PatchError, parse_patch
from .text import (STDIN, TextError, display_name, read_bytes, read_lines, read_text, write_bytes, write_lines,
                   write_text)
from .unified import hunk_header, render

PROG = "run.py"
USAGE = ("usage: run.py diff [-q] [-N] [-i] [-b | -w] [-u | -U N] [-L LABEL]... OLD NEW | "
         "run.py patch [-F N] [-R] [-p N] [-o OUT] [-r REJ] [-b] [-s] [--dry-run] [FILE] PATCHFILE | "
         "run.py stat [-R] PATCHFILE")
NUMBER = re.compile(r"[0-9]+\Z")
DEV_NULL = "/dev/null"


class UsageError(Exception):
    """A command line the program does not accept."""


class Option(object):
    """One option: its short and long spellings, the key it sets and whether it takes a value."""
    __slots__ = ("short", "long", "key", "takes_value")

    def __init__(self, short, long, key, takes_value):
        self.short = short
        self.long = long
        self.key = key
        self.takes_value = takes_value


DIFF_OPTIONS = [
    Option("-U", "--unified", "context", True),
    Option("-u", None, "default_context", False),
    Option("-L", "--label", "label", True),
    Option("-q", "--brief", "brief", False),
    Option("-N", "--new-file", "new_file", False),
    Option("-i", "--ignore-case", "ignore_case", False),
    Option("-b", "--ignore-space-change", "space_change", False),
    Option("-w", "--ignore-all-space", "all_space", False),
]
PATCH_OPTIONS = [
    Option("-F", "--fuzz", "fuzz", True),
    Option("-R", "--reverse", "reverse", False),
    Option("-p", "--strip", "strip", True),
    Option("-o", "--output", "output", True),
    Option("-r", "--reject-file", "reject_file", True),
    Option("-b", "--backup", "backup", False),
    Option("-s", "--silent", "quiet", False),
    Option(None, "--quiet", "quiet", False),
    Option(None, "--dry-run", "dry_run", False),
]
STAT_OPTIONS = [
    Option("-R", "--reverse", "reverse", False),
]


def find_option(options, arg):
    """The option an argument names and the value attached to it (or None)."""
    for option in options:
        if option.long is not None and (arg == option.long or arg.startswith(option.long + "=")):
            if arg == option.long:
                return option, None
            if not option.takes_value:
                raise UsageError("option %s takes no value" % option.long)
            return option, arg[len(option.long) + 1:]
        if option.short is not None and arg.startswith(option.short):
            rest = arg[len(option.short):]
            if not rest:
                return option, None
            if option.takes_value:
                return option, rest
    raise UsageError("unknown option %s" % arg)


def parse_args(args, options):
    """Splits the arguments into ([(key, value)] in order, operands). Options and operands may be mixed; `--`
    ends the options and a lone `-` is an operand."""
    settings, operands = [], []
    index = 0
    while index < len(args):
        arg = args[index]
        index += 1
        if arg == "--":
            operands.extend(args[index:])
            break
        if arg == STDIN or not arg.startswith("-"):
            operands.append(arg)
            continue
        option, value = find_option(options, arg)
        if option.takes_value and value is None:
            if index == len(args):
                raise UsageError("option %s needs a value" % arg)
            value = args[index]
            index += 1
        settings.append((option.key, value if option.takes_value else True))
    return settings, operands


def parse_count(value, what):
    if not NUMBER.match(value):
        raise UsageError("invalid %s %r" % (what, value))
    return int(value)


def output(text):
    sys.stdout.buffer.write(text.encode("utf-8"))


class DiffSettings(object):
    __slots__ = ("context", "labels", "brief", "new_file", "ignore_case", "space_change", "all_space")

    def __init__(self, settings):
        self.context, self.labels = DEFAULT_CONTEXT, []
        self.brief = self.new_file = self.ignore_case = self.space_change = self.all_space = False
        for key, value in settings:
            if key == "context":
                self.context = parse_count(value, "context length")
            elif key == "default_context":
                self.context = DEFAULT_CONTEXT
            elif key == "label":
                self.labels.append(value)
            else:
                setattr(self, key, True)
        if len(self.labels) > 2:
            raise UsageError("at most two labels")

    def compared(self):
        """The comparison function of the ignore options, or None when lines are compared as they are."""
        if not (self.ignore_case or self.space_change or self.all_space):
            return None
        return comparison(self.ignore_case, self.space_change, self.all_space)


def run_diff(args):
    settings, operands = parse_args(args, DIFF_OPTIONS)
    opts = DiffSettings(settings)
    if len(operands) != 2:
        raise UsageError("diff needs two files")
    if operands == [STDIN, STDIN]:
        raise UsageError("standard input can be compared only once")
    old_path, new_path = operands
    if opts.new_file and not (os.path.lexists(old_path) or os.path.lexists(new_path)):
        raise TextError("%s: No such file or directory" % old_path)
    old = read_lines(old_path, opts.new_file)
    new = read_lines(new_path, opts.new_file)
    hunks = make_hunks(old, new, opts.context, opts.compared())
    if not hunks:
        return 0
    if opts.brief:
        output("Files %s and %s differ\n" % (old_path, new_path))
        return 1
    labels = opts.labels + operands[len(opts.labels):]
    output(render(labels[0], labels[1], hunks))
    return 1


class PatchSettings(object):
    __slots__ = ("fuzz", "strip", "reverse", "output", "reject_file", "backup", "quiet", "dry_run")

    def __init__(self, settings):
        self.fuzz, self.strip = None, 0
        self.reverse = self.dry_run = self.backup = self.quiet = False
        self.output = self.reject_file = None
        for key, value in settings:
            if key == "fuzz":
                self.fuzz = parse_count(value, "fuzz factor")
            elif key == "strip":
                self.strip = parse_count(value, "strip count")
            elif key in ("output", "reject_file"):
                setattr(self, key, value)
            else:
                setattr(self, key, True)


def first_line(path, target, dry_run):
    """The first output line of a patch run (SPEC.md 8.4)."""
    verb = "checking" if dry_run else "patching"
    if target == path:
        return "%s file %s" % (verb, path)
    return "%s file %s (read from %s)" % (verb, target, path)


def save_backup(target):
    """Keeps the previous content of the file about to be written as TARGET.orig (SPEC.md 8.5)."""
    if os.path.lexists(target):
        write_bytes(target + ".orig", read_bytes(target))


def file_from_labels(patch, strip):
    """The file a patch names (SPEC.md 8.1): its +++ label, or its --- label when the +++ one is /dev/null,
    without its first `strip` path components."""
    label = patch.old_label if patch.new_label == DEV_NULL else patch.new_label
    parts = label.split("/")
    if label == DEV_NULL or not label or len(parts) <= strip:
        raise TextError("cannot tell the file to patch from the label %r" % label)
    return "/".join(parts[strip:])


def read_patch(patch_path, reverse):
    """The file sections of a patch file (exchanged with -R)."""
    try:
        sections = parse_patch(read_text(patch_path))
    except PatchError as e:
        raise TextError("%s: malformed patch at %s" % (display_name(patch_path), e))
    return [section.reversed() for section in sections] if reverse else sections


class Job(object):
    """One file section to apply: the file it patches, where the result and the rejects go, and its lines."""
    __slots__ = ("section", "path", "target", "reject_path", "lines")

    def __init__(self, section, path, opts):
        self.section = section
        self.path = path
        self.target = opts.output or path
        self.reject_path = opts.reject_file or self.target + ".rej"
        self.lines = read_lines(path)


def plan_jobs(sections, operands, opts):
    """Pairs every file section with its file (SPEC.md 8.1) and reads all the files before anything is applied."""
    if len(operands) == 2:
        if len(sections) > 1:
            raise UsageError("a FILE operand needs a patch with one file section")
        return [Job(sections[0], operands[0], opts)]
    if len(sections) > 1 and (opts.output or opts.reject_file):
        raise UsageError("-o and -r need a patch with one file section")
    jobs, seen = [], set()
    for section in sections:
        path = file_from_labels(section, opts.strip)
        if path in seen:
            raise UsageError("two file sections patch %s" % path)
        seen.add(path)
        jobs.append(Job(section, path, opts))
    return jobs


def run_job(job, opts, fuzz, report):
    """Applies one file section, adds its output lines to report and writes its files; True when a hunk failed."""
    outcome = apply_hunks(job.lines, job.section.hunks, fuzz)
    report.append(first_line(job.path, job.target, opts.dry_run))
    report.extend(outcome.messages)
    if outcome.rejected:
        report.append(summary(outcome, job.reject_path, opts.dry_run))
    if not opts.dry_run:
        if opts.backup:
            save_backup(job.target)
        write_lines(job.target, outcome.lines)
        if outcome.rejected:
            write_text(job.reject_path, reject_text(job.section, outcome))
    return bool(outcome.rejected)


def run_patch(args):
    settings, operands = parse_args(args, PATCH_OPTIONS)
    opts = PatchSettings(settings)
    if len(operands) not in (1, 2):
        raise UsageError("patch needs a patch and at most one file")
    patch_path = operands[-1]
    if operands[0] == STDIN and len(operands) == 2:
        raise UsageError("the file to patch cannot be standard input")
    sections = read_patch(patch_path, opts.reverse)
    jobs = plan_jobs(sections, operands, opts)
    fuzz = opts.fuzz or DEFAULT_FUZZ
    report = []
    failed = False
    for job in jobs:
        if run_job(job, opts, fuzz, report):
            failed = True
    if not opts.quiet:
        output("".join(line + "\n" for line in report))
    return 1 if failed else 0


def plural(count, word):
    return "%d %s%s" % (count, word, "" if count == 1 else "s")


def hunk_stat(hunk):
    """One line of the stat command (SPEC.md 9.2): the canonical hunk header and its change counts."""
    deleted = sum(1 for tag, _ in hunk.lines if tag == DELETED)
    inserted = sum(1 for tag, _ in hunk.lines if tag == INSERTED)
    return "%s -%d +%d" % (hunk_header(hunk), deleted, inserted), deleted, inserted


def run_stat(args):
    settings, operands = parse_args(args, STAT_OPTIONS)
    if len(operands) != 1:
        raise UsageError("stat needs one patch")
    sections = read_patch(operands[0], bool(settings))
    report = []
    hunks = deleted = inserted = 0
    for section in sections:
        report.append("%s -> %s" % (section.old_label, section.new_label))
        for hunk in section.hunks:
            line, d, i = hunk_stat(hunk)
            report.append(line)
            deleted += d
            inserted += i
        hunks += len(section.hunks)
    totals = "%s, %s, %s" % (plural(hunks, "hunk"), plural(deleted, "deletion"), plural(inserted, "insertion"))
    if len(sections) > 1:
        totals = "%s, %s" % (plural(len(sections), "file"), totals)
    report.append(totals)
    output("".join(line + "\n" for line in report))
    return 0


COMMANDS = {"diff": run_diff, "patch": run_patch, "stat": run_stat}


def main(argv):
    if not argv or argv[0] not in COMMANDS:
        sys.stderr.write("%s: unknown command (%s)\n" % (PROG, USAGE))
        return 2
    try:
        return COMMANDS[argv[0]](argv[1:])
    except UsageError as e:
        sys.stderr.write("%s: %s (%s)\n" % (PROG, e, USAGE))
        return 2
    except TextError as e:
        sys.stderr.write("%s: %s\n" % (PROG, e))
        return 2
