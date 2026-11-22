#!/usr/bin/env python3
"""Guard the doc pages that list the gate's steps.

The gate's ordered sequence lives in one place — ``GATE_SEQUENCE`` in
``scripts/krkr_gates/cli.py``, the registry the gate dispatches from — and
several pages repeat it by hand: the ordered table in
``docs/rules/gates.md`` §1, the table in ``docs/rules/code_styles.md`` §10,
and the sequence prose in ``scripts/README.md``.  A list kept in five places
drifts, and the drift is invisible: the page that describes the gate stops
describing the gate, and nothing fails.

This check reads the code and compares the pages with it:

* every step of ``GATE_SEQUENCE`` appears in the ``gates.md`` table, in the
  same order, with the same kind, and the table names no step the gate does
  not run;
* every check of ``GATE_SEQUENCE`` appears in the ``code_styles.md`` table,
  and that table names no step the gate does not run.

A missing, extra, reordered or mistyped entry is a finding with the file and
the line.  The check never writes: the pages are edited by hand, and a guard
that rewrote them would hide the drift it exists to catch.

The step list is read from the registry itself — the file the gate dispatches
from — so the check never carries a second copy of it.  The fixture corpus
under ``scripts/fixtures/doc_lists/`` holds a good tree (the pages and the
registry agree) and a bad one (they disagree in every way the rules can see),
and the self-test proves that each rule fires on the bad tree and that the
good tree stays clean.

Usage:
    uv run --project scripts krkr-gate doc-lists
    uv run --project scripts krkr-gate doc-lists --self-test
"""

from __future__ import annotations

import ast
import re
import sys
from pathlib import Path
from typing import NamedTuple

from ..common import FIXTURES_DIR, REPO_ROOT
from ..framework import (
    Finding,
    FixtureRules,
    dedupe_findings,
    fixture_problems,
    report_self_test_failure,
)

ROOT = REPO_ROOT
FIXTURES = FIXTURES_DIR / "doc_lists"
GOOD_TREE = FIXTURES / "good"
BAD_TREE = FIXTURES / "bad"

#: The registry the gate dispatches from, and the pages that describe it.
CLI_PATH = "scripts/krkr_gates/cli.py"
GATES_PATH = "docs/rules/gates.md"
STYLES_PATH = "docs/rules/code_styles.md"

#: The section of each page that holds the table.
GATES_SECTION = ("1", "The ordered static gate")
STYLES_SECTION = ("10", "What the gate enforces")
#: The header cells of each table.
GATES_HEADER = ("#", "Step", "Kind", "What it protects")
STYLES_HEADER = ("check", "the rule it protects")

#: A step of the gate has no row in the ordered table.
RULE_MISSING = "gate-step-missing"
#: The ordered table names a step the gate does not run.
RULE_EXTRA = "gate-step-extra"
#: The ordered table lists the steps in another order than the gate runs them.
RULE_ORDER = "gate-step-order"
#: The ordered table gives a step another kind than the gate runs it with.
RULE_KIND = "gate-step-kind"
#: A check of the gate is not named in the "what the gate enforces" table.
RULE_DOC_MISSING = "doc-check-missing"
#: The "what the gate enforces" table names a step the gate does not run.
RULE_DOC_EXTRA = "doc-check-extra"

_HEADING = re.compile(r"^##\s+(?P<number>\d+)\.\s+(?P<title>.*\S)\s*$")


class Step(NamedTuple):
    """One step of the gate, as the registry declares it."""

    label: str
    kind: str
    name: str
    argv: list[str]

    @property
    def command(self) -> str:
        """The command the gate runs, as the doc pages state it."""
        if self.kind == "cargo":
            return " ".join(self.argv)
        return " ".join([self.name, *self.argv])


def _relative(path: Path, root: Path) -> str:
    """The repo-relative path of a scanned file, or the root-relative one."""
    try:
        return path.relative_to(ROOT).as_posix()
    except ValueError:
        return path.relative_to(root).as_posix()


def _plain(text: str) -> str:
    """A table cell's text without its backticks and with single spaces."""
    return " ".join(text.replace("`", "").split())


def gate_steps(cli_path: Path) -> tuple[list[Step], int]:
    """The gate's ordered steps, read from the registry the gate dispatches from.

    The registry is read as the source it is — a literal list of tuples — so
    the check follows the code instead of a copy of it.  A registry that is no
    longer a literal cannot be read this way, and that is a hard error rather
    than a finding: the gate would not run what the check checked.
    """
    source = cli_path.read_text(encoding="utf-8")
    try:
        tree = ast.parse(source)
    except SyntaxError as exc:
        raise SystemExit(
            f"krkr-gate: {cli_path} does not parse ({exc}); the gate's steps "
            "cannot be read"
        ) from exc
    for node in ast.walk(tree):
        if isinstance(node, ast.AnnAssign):
            target = node.target
        elif isinstance(node, ast.Assign):
            target = node.targets[0] if node.targets else None
        else:
            continue
        if not isinstance(target, ast.Name) or target.id != "GATE_SEQUENCE":
            continue
        try:
            value = ast.literal_eval(node.value)
        except ValueError as exc:
            raise SystemExit(
                f"krkr-gate: {cli_path}:{node.lineno}: GATE_SEQUENCE is not a "
                f"literal ({exc}); the gate's steps cannot be read"
            ) from exc
        steps = [Step(*entry) for entry in value]
        return steps, node.lineno
    raise SystemExit(
        f"krkr-gate: {cli_path} declares no GATE_SEQUENCE; the gate's steps "
        "cannot be read"
    )


def _section(lines: list[str], number: str, title: str) -> list[tuple[int, str]]:
    """The lines of the ``## <number>. <title>`` section, with their numbers."""
    start = None
    for index, line in enumerate(lines):
        match = _HEADING.match(line)
        if match and match.group("number") == number and match.group("title") == title:
            start = index
            break
    if start is None:
        return []
    end = len(lines)
    for index in range(start + 1, len(lines)):
        if _HEADING.match(lines[index]):
            end = index
            break
    return [(index + 1, lines[index]) for index in range(start + 1, end)]


def _cells(line: str) -> list[str]:
    """The cells of a markdown table row."""
    text = line.strip()
    if text.startswith("|"):
        text = text[1:]
    if text.endswith("|"):
        text = text[:-1]
    return [cell.strip() for cell in text.split("|")]


def _is_separator(cells: list[str]) -> bool:
    """Whether a row is the table's ``|---|---|`` separator."""
    return all(cell and set(cell) <= set("-: ") for cell in cells)


def _table(
    section: list[tuple[int, str]], header: tuple[str, ...]
) -> tuple[int, list[tuple[int, list[str]]]] | None:
    """The header line and the body rows of the table with that header."""
    for position, (number, line) in enumerate(section):
        if _cells(line) != list(header):
            continue
        rows: list[tuple[int, list[str]]] = []
        for follow_number, follow_line in section[position + 1 :]:
            if not follow_line.strip().startswith("|"):
                break
            cells = _cells(follow_line)
            if _is_separator(cells):
                continue
            rows.append((follow_number, cells))
        return number, rows
    return None


def compare_gates(steps: list[Step], path: Path, root: Path) -> list[Finding]:
    """The findings between the gate's steps and the ordered table."""
    findings: list[Finding] = []
    rel = _relative(path, root)
    lines = path.read_text(encoding="utf-8").splitlines()
    section = _section(lines, *GATES_SECTION)
    if not section:
        return [
            Finding(
                rel,
                1,
                RULE_MISSING,
                "the ordered table",
                f"section {GATES_SECTION[0]} ({GATES_SECTION[1]}) is not in "
                f"{rel}; the gate's steps cannot be listed there",
            )
        ]
    table = _table(section, GATES_HEADER)
    if table is None:
        return [
            Finding(
                rel,
                section[0][0],
                RULE_MISSING,
                "the ordered table",
                f"no table with the header {' | '.join(GATES_HEADER)} in "
                f"section {GATES_SECTION[0]} of {rel}",
            )
        ]
    header_line, rows = table
    step_column = GATES_HEADER.index("Step")
    kind_column = GATES_HEADER.index("Kind")

    by_command: dict[str, int] = {}
    for index, step in enumerate(steps):
        by_command.setdefault(step.command, index)

    row_for: dict[str, tuple[int, list[str]]] = {}
    for number, cells in rows:
        row_for.setdefault(_plain(cells[step_column]), (number, cells))

    for step in steps:
        if step.command in row_for:
            continue
        findings.append(
            Finding(
                rel,
                header_line,
                RULE_MISSING,
                step.command,
                f"the gate runs `{step.command}`, the ordered table has no row "
                "for it",
            )
        )

    for number, cells in rows:
        command = _plain(cells[step_column])
        if command in by_command:
            continue
        findings.append(
            Finding(
                rel,
                number,
                RULE_EXTRA,
                command,
                f"the ordered table names `{command}`, the gate runs no such "
                "step",
            )
        )

    for step in steps:
        row = row_for.get(step.command)
        if row is None:
            continue
        number, cells = row
        kind = _plain(cells[kind_column]).lower()
        if kind == step.kind:
            continue
        findings.append(
            Finding(
                rel,
                number,
                RULE_KIND,
                kind,
                f"the ordered table calls `{step.command}` a `{kind}` step, "
                f"the gate runs it as `{step.kind}`",
            )
        )

    listed: list[tuple[int, Step]] = []
    for number, cells in rows:
        command = _plain(cells[step_column])
        index = by_command.get(command)
        if index is None:
            continue
        listed.append((number, steps[index]))
    for (previous_line, previous), (number, current) in zip(listed, listed[1:]):
        if by_command[current.command] <= by_command[previous.command]:
            findings.append(
                Finding(
                    rel,
                    number,
                    RULE_ORDER,
                    current.command,
                    f"the ordered table lists `{current.command}` after "
                    f"`{previous.command}`, the gate runs it before",
                )
            )
            break
    return findings


def compare_styles(steps: list[Step], path: Path, root: Path) -> list[Finding]:
    """The findings between the gate's checks and the "what it protects" table."""
    findings: list[Finding] = []
    rel = _relative(path, root)
    lines = path.read_text(encoding="utf-8").splitlines()
    section = _section(lines, *STYLES_SECTION)
    if not section:
        return [
            Finding(
                rel,
                1,
                RULE_DOC_MISSING,
                "the table of checks",
                f"section {STYLES_SECTION[0]} ({STYLES_SECTION[1]}) is not in "
                f"{rel}; the gate's checks cannot be listed there",
            )
        ]
    table = _table(section, STYLES_HEADER)
    if table is None:
        return [
            Finding(
                rel,
                section[0][0],
                RULE_DOC_MISSING,
                "the table of checks",
                f"no table with the header {' | '.join(STYLES_HEADER)} in "
                f"section {STYLES_SECTION[0]} of {rel}",
            )
        ]
    header_line, rows = table

    gate_checks = {step.name for step in steps if step.kind == "check"}
    gate_cargo = {step.command for step in steps if step.kind == "cargo"}

    named: dict[str, int] = {}
    entries: list[tuple[int, str, str]] = []
    for number, cells in rows:
        for cell in cells[0].split(","):
            text = _plain(cell)
            if not text:
                continue
            token = text.split(" ")[0]
            entries.append((number, text, token))
            named.setdefault(token, number)

    for name in sorted(gate_checks - set(named)):
        findings.append(
            Finding(
                rel,
                header_line,
                RULE_DOC_MISSING,
                name,
                f"the gate runs the check `{name}`, the table of what the gate "
                "enforces does not name it",
            )
        )

    seen: set[tuple[int, str]] = set()
    for number, text, token in entries:
        if token in gate_checks:
            continue
        if text in gate_cargo:
            continue
        if (number, text) in seen:
            continue
        seen.add((number, text))
        findings.append(
            Finding(
                rel,
                number,
                RULE_DOC_EXTRA,
                text,
                f"the table of what the gate enforces names `{text}`, the gate "
                "runs no such step",
            )
        )
    return findings


def scan(root: Path = ROOT) -> list[Finding]:
    """Every drift between the gate's steps and the pages that list them."""
    steps, _line = gate_steps(root / CLI_PATH)
    findings = compare_gates(steps, root / GATES_PATH, root)
    findings += compare_styles(steps, root / STYLES_PATH, root)
    return dedupe_findings(findings)


GOOD_GATES = "scripts/fixtures/doc_lists/good/docs/rules/gates.md"
GOOD_STYLES = "scripts/fixtures/doc_lists/good/docs/rules/code_styles.md"
GOOD_CLI = "scripts/fixtures/doc_lists/good/scripts/krkr_gates/cli.py"
BAD_GATES = "scripts/fixtures/doc_lists/bad/docs/rules/gates.md"
BAD_STYLES = "scripts/fixtures/doc_lists/bad/docs/rules/code_styles.md"

# Each group: the bad fixture must trip every `rules` entry and the good
# fixture must trip nothing at all.
SELF_TEST_GROUPS = (
    FixtureRules(
        BAD_GATES,
        (RULE_MISSING, RULE_EXTRA, RULE_ORDER, RULE_KIND),
        (GOOD_GATES, GOOD_STYLES, GOOD_CLI),
    ),
    FixtureRules(
        BAD_STYLES,
        (RULE_DOC_MISSING, RULE_DOC_EXTRA),
        (GOOD_GATES, GOOD_STYLES, GOOD_CLI),
    ),
)


def self_test() -> int:
    """Assert each rule still matches its bad fixture and not its near-miss.

    The bad tree drifts from the registry in every way the rules can see — a
    missing step, an extra step, a reordered step, a mistyped kind, a check
    the table of checks does not name and a step it names wrongly — and the
    good tree agrees with it in every way.  A rule that stops matching fails
    the gate instead of passing it silently.
    """
    for tree in (GOOD_TREE, BAD_TREE):
        if not tree.is_dir():
            print(f"ERROR: doc-lists self-test fixtures missing at {tree}")
            return 1
    findings = scan(GOOD_TREE) + scan(BAD_TREE)
    problems = fixture_problems(findings, SELF_TEST_GROUPS)
    required_total = sum(len(group.rules) for group in SELF_TEST_GROUPS)
    if problems:
        report_self_test_failure("ERROR: doc-lists self-test failed", problems)
        return 1
    print(
        "doc-lists guard: self-test ok "
        f"({required_total} required rule flag(s) matched their bad fixture, "
        "0 finding(s) in the good fixtures)"
    )
    return 0


def main(argv: list[str] | None = None) -> int:
    selftest = self_test()
    if selftest != 0:
        return selftest
    if "--self-test" in (argv or []):
        return 0

    findings = scan(ROOT)
    for finding in findings:
        print(
            f"ERROR: gate-step drift [{finding.rule}] "
            f"{finding.path}:{finding.line}: {finding.message}"
        )
    if findings:
        print("")
        print(
            f"ERROR: {len(findings)} drift(s) between the gate's steps and the "
            "pages that list them."
        )
        print("  The gate's step list is GATE_SEQUENCE in scripts/krkr_gates/cli.py;")
        print("  the pages must state the steps the gate runs, in that order.")
        return 1
    print(
        "doc-lists guard: clean — the gate's steps and the pages that list "
        "them agree"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
