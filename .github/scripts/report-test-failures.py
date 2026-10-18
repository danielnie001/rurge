#!/usr/bin/env python3
"""Turns a captured `cargo test` log into GitHub annotations and a job summary.

A job's log can only be read by someone signed in to GitHub; its annotations
and its summary can be read by anyone. So when the test step fails, this
script repeats what failed — each failed test with the head of its output,
each test binary that exited abnormally, each compile error, and, when the
log ends inside a test binary, the tests still running and the ones that had
failed by then — where it can be seen without the log.

Usage: report-test-failures.py <log file>

Always exits 0: the job has failed already, and a report that cannot be made
must not hide that behind a second failure.
"""

import os
import re
import sys

# GitHub keeps 10 annotations of each level per step.
PER_LEVEL = 10
LEVELS = ("error", "warning", "notice")
# One annotation: enough to hold a panic message and the head of what the
# test printed, short enough that the first of them is not cut off.
MAX_LINES = 60
MAX_CHARS = 3000

# The toolchain action sets `CARGO_TERM_COLOR=always` for the whole job, so
# what cargo writes — `Running …`, `error: test failed, …` — comes wrapped in
# colour codes, and a link in `Finished …`. They are taken off every line
# before it is read: control sequences (`ESC [ … letter`) and operating
# system commands (`ESC ] … BEL` or `ESC ] … ESC \`).
ESCAPE = re.compile(r"\x1b\[[0-?]*[ -/]*[@-~]|\x1b\][^\x07\x1b]*(?:\x07|\x1b\\)")
RUNNING = re.compile(r"^\s+Running (?:unittests )?(\S+) \((.+)\)\s*$")
DOCTEST = re.compile(r"^\s+Doc-tests (\S+)\s*$")
BLOCK = re.compile(r"^---- (.+?) (?:stdout|stderr) ----\s*$")
RESULT = re.compile(r"^test result: (\w+)\. .*$")
RERUN = re.compile(r"^error: test failed, to rerun pass `(.+)`\s*$")
SLOW = re.compile(r"^test (\S+) has been running for over \d+ seconds\s*$")
DONE = re.compile(r"^test (\S+)(?: - .+)? \.\.\. (\w+)")
COMPILE = re.compile(r"^error(\[E\d+\])?: ")
BACKTRACE_NOTE = "note: run with `RUST_BACKTRACE=1`"


class Binary:
    """One test executable's stretch of the log."""

    def __init__(self, name):
        self.name = name
        self.rerun = None
        self.result = None
        self.failed = []  # (test name, its output)
        self.crash = None
        self.slow = []
        self.done = set()
        # libtest prints a failed test's status line when it ends, but its
        # output only when the binary does: a binary that hangs keeps these
        self.said_failed = []

    def label(self):
        return self.rerun or self.name


def parse(lines):
    """Returns the test binaries in log order and the compile errors."""
    binaries = []
    compile_errors = []
    current = None
    block = None  # (test name, lines) while inside a `---- name stdout ----`
    i = 0
    while i < len(lines):
        line = lines[i]
        started = RUNNING.match(line) or DOCTEST.match(line)
        if started:
            close(current, block)
            block = None
            current = Binary(named(started))
            binaries.append(current)
            i += 1
            continue
        if current is not None:
            opened = BLOCK.match(line)
            if opened:
                close(current, block)
                block = (opened.group(1), [])
                i += 1
                continue
            if block is not None and (line.strip() == "failures:" or RESULT.match(line)):
                close(current, block)
                block = None
            if block is not None:
                block[1].append(line)
                i += 1
                continue
            result = RESULT.match(line)
            if result:
                current.result = line.strip()
            slow = SLOW.match(line)
            if slow:
                current.slow.append(slow.group(1))
            done = DONE.match(line)
            if done:
                current.done.add(done.group(1))
                if done.group(2) == "FAILED":
                    current.said_failed.append(done.group(1))
            rerun = RERUN.match(line)
            if rerun:
                current.rerun = rerun.group(1)
                # An abnormal exit says why two lines further down.
                for follow in lines[i + 1 : i + 5]:
                    if "process didn't exit successfully" in follow:
                        current.crash = follow.strip()
                i += 1
                continue
        if current is None and COMPILE.match(line):
            # Only before the first test binary: the tests of the `rurge`
            # binary print diagnostics of the same shape (`error[E0022]: …`).
            # The error with what follows it, up to the next blank line.
            chunk = [line]
            j = i + 1
            while j < len(lines) and lines[j].strip() and len(chunk) < 25:
                chunk.append(lines[j])
                j += 1
            compile_errors.append("\n".join(chunk))
            i = j
            continue
        i += 1
    close(current, block)
    return binaries, compile_errors


def named(started):
    """`src/lib.rs [rurge_net]`: the source alone is the same for every crate."""
    if started.re is DOCTEST:
        return "doc-tests " + started.group(1)
    executable = re.split(r"[\\/]", started.group(2))[-1]
    stem = re.sub(r"(-[0-9a-f]{16})?(\.exe)?$", "", executable)
    return "%s [%s]" % (started.group(1), stem)


def close(binary, block):
    if binary is not None and block is not None:
        binary.failed.append((block[0], tidy(block[1])))


def tidy(lines):
    """A failed test's output without the blank edges and the backtrace hint."""
    kept = [line for line in lines if not line.startswith(BACKTRACE_NOTE)]
    while kept and not kept[0].strip():
        kept.pop(0)
    while kept and not kept[-1].strip():
        kept.pop()
    return "\n".join(kept)


def clip(text):
    lines = text.split("\n")
    more = 0
    if len(lines) > MAX_LINES:
        more = len(lines) - MAX_LINES
        lines = lines[:MAX_LINES]
    out = "\n".join(lines)
    if len(out) > MAX_CHARS:
        out = out[:MAX_CHARS] + "\n… (cut)"
    elif more:
        out += "\n… (%d more lines)" % more
    return out


def findings(binaries, compile_errors):
    """(title, text) pairs, the ones that explain the most first."""
    found = [("compile error", error) for error in compile_errors]
    for binary in binaries:
        if binary.crash:
            found.append((binary.label() + " exited abnormally", binary.crash))
    last = binaries[-1] if binaries else None
    if last is not None and last.result is None and not last.crash:
        # The log ends inside this binary: it hung, or the step was cancelled.
        waiting = [name for name in last.slow if name not in last.done]
        text = "the log ends before this test binary reported a result"
        if waiting:
            text += "; still running: " + ", ".join(waiting)
        if last.said_failed:
            text += "; failed before the log ends: " + ", ".join(last.said_failed)
        found.append((last.label() + " did not finish", text))
    for binary in binaries:
        for name, output in binary.failed:
            found.append((binary.label() + " :: " + name, output or "(no output)"))
    return found


def data(text):
    return text.replace("%", "%25").replace("\r", "%0D").replace("\n", "%0A")


def prop(text):
    return data(text).replace(":", "%3A").replace(",", "%2C")


def annotate(found):
    room = PER_LEVEL * len(LEVELS) - 1  # the last one says what did not fit
    for index, (title, text) in enumerate(found[:room]):
        level = LEVELS[index // PER_LEVEL]
        print("::%s title=%s::%s" % (level, prop(title[:200]), data(clip(text))))
    rest = found[room:]
    if rest:
        names = "\n".join(title for title, _ in rest)
        print("::notice title=%s::%s" % (prop("%d more failures" % len(rest)), data(clip(names))))


def summarise(found, binaries):
    out = ["## Failed tests", ""]
    if not found:
        out.append("The log names no failed test, abnormal exit or compile error.")
    for title, text in found:
        out += ["<details><summary>%s</summary>" % title, "", "````text", text, "````", "", "</details>", ""]
    failed = [b for b in binaries if b.result and "FAILED" in b.result]
    if failed:
        out += ["### Results", ""]
        out += ["- `%s`: %s" % (b.label(), b.result) for b in failed]
    return "\n".join(out) + "\n"


def main():
    if len(sys.argv) != 2:
        print("usage: report-test-failures.py <log file>")
        return
    try:
        with open(sys.argv[1], encoding="utf-8", errors="replace") as log:
            lines = [ESCAPE.sub("", line.rstrip("\r\n")) for line in log]
    except OSError as error:
        print("::notice title=No test log::%s" % data(str(error)))
        return
    binaries, compile_errors = parse(lines)
    found = findings(binaries, compile_errors)
    if not found:
        print("::notice title=No failed test found in the log::%d test binaries ran" % len(binaries))
    annotate(found)
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        with open(summary, "a", encoding="utf-8") as out:
            out.write(summarise(found, binaries))


if __name__ == "__main__":
    main()
