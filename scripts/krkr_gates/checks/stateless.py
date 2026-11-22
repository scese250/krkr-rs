"""Static guard for process-global mutable state in the bottom-core crates.

Checks the bottom crates for process-global mutable state, hidden caches and
singletons, and checks the service objects for stateful (``&mut self``)
methods. The rules are the same literal patterns and service list as the
original shell guard.

The guard runs its own fixture corpus on every invocation: a broken sample
that every rule must catch and a clean sample that must stay clean.  A rule
that stops matching fails the gate instead of passing it silently, which is
the difference between a guard and a comment.

Usage:
    uv run --project scripts krkr-gate stateless
    uv run --project scripts krkr-gate stateless --self-test
"""

from __future__ import annotations

import re
from collections.abc import Iterable
from pathlib import Path

from ..common import FIXTURES_DIR, REPO_ROOT
from ..framework import (
    Finding,
    FixtureRules,
    fixture_problems,
    group_findings,
    report_self_test_failure,
)

ROOT = REPO_ROOT

# Bottom-core crates. The middle layer (engine) owns state and is excluded.
CRATES = (
    "crates/platform/api",
    "crates/platform/fs",
    "crates/platform/media",
    "crates/platform/render",
    "crates/platform/platform",
)

# Forbidden patterns, matched against .rs sources.  Each pattern is a rule of
# its own: the self-test proves that every one of them still fires.
PATTERNS = (
    ("static-mut", re.compile(r"static\s+mut\s")),
    ("thread-local", re.compile(r"thread_local!")),
    ("lazy-static", re.compile(r"lazy_static!")),
    ("lazy-lock", re.compile(r"LazyLock")),
    ("once-lock", re.compile(r"OnceLock")),
    ("once-cell", re.compile(r"OnceCell")),
)

# Service objects that must be immutable after construction.
SERVICES = (
    ("crates/platform/render", "RenderPipeline"),
    ("crates/platform/fs", "FileSystem"),
    ("crates/platform/media", "Media"),
)

#: The rule a stateful service method is reported under.
RULE_STATEFUL = "stateful-service"
#: The rules the forbidden global patterns are reported under.
GLOBAL_RULES = tuple(name for name, _pattern in PATTERNS)

_IMPL = re.compile(r"^[ \t]*impl[ \t]")
_PUB_MUT_SELF = re.compile(r"pub fn[ \t]+[A-Za-z_0-9]+[ \t]*\(&mut self")
_CLOSE = re.compile(r"^}")

FIXTURES = FIXTURES_DIR / "stateless"
TREE = FIXTURES / "tree"
BAD_GLOBALS = "scripts/fixtures/stateless/tree/crates/platform/api/src/globals.rs"
GOOD_GLOBALS = "scripts/fixtures/stateless/tree/crates/platform/api/src/clean.rs"
BAD_RENDER = "scripts/fixtures/stateless/tree/crates/platform/render/src/pipeline.rs"
GOOD_RENDER = "scripts/fixtures/stateless/tree/crates/platform/render/src/clean.rs"
BAD_FILES = "scripts/fixtures/stateless/tree/crates/platform/fs/src/service.rs"
GOOD_FILES = "scripts/fixtures/stateless/tree/crates/platform/fs/src/clean.rs"
BAD_MEDIA = "scripts/fixtures/stateless/tree/crates/platform/media/src/service.rs"
GOOD_MEDIA = "scripts/fixtures/stateless/tree/crates/platform/media/src/clean.rs"

# Each group: the bad fixture must trip every `rules` entry and the good
# fixture must trip nothing at all.  Every service the guard knows is covered,
# so a type name the guard stopped matching fails the self-test.
SELF_TEST_GROUPS = (
    FixtureRules(BAD_GLOBALS, GLOBAL_RULES, (GOOD_GLOBALS,)),
    FixtureRules(BAD_RENDER, (RULE_STATEFUL,), (GOOD_RENDER,)),
    FixtureRules(BAD_FILES, (RULE_STATEFUL,), (GOOD_FILES,)),
    FixtureRules(BAD_MEDIA, (RULE_STATEFUL,), (GOOD_MEDIA,)),
)


def _rust_sources(root: Path, crate: str) -> Iterable[Path]:
    src = root / crate / "src"
    if not src.is_dir():
        return ()
    return sorted(src.rglob("*.rs"))


def _relative(path: Path, root: Path) -> str:
    """The repo-relative path of a scanned file, or the root-relative one."""
    try:
        return path.relative_to(ROOT).as_posix()
    except ValueError:
        return path.relative_to(root).as_posix()


def scan_globals(root: Path = ROOT) -> list[Finding]:
    """Return one finding per forbidden process-global pattern."""
    findings: list[Finding] = []
    for crate in CRATES:
        for rule, pattern in PATTERNS:
            for path in _rust_sources(root, crate):
                rel = _relative(path, root)
                text = path.read_text(encoding="utf-8", errors="replace")
                for lineno, line in enumerate(text.splitlines(), 1):
                    if pattern.search(line):
                        findings.append(Finding(rel, lineno, rule, line.strip(), crate))
    return findings


def scan_services(root: Path = ROOT) -> list[Finding]:
    """Return one finding per stateful (``&mut self``) service method."""
    findings: list[Finding] = []
    for crate, type_name in SERVICES:
        for path in _rust_sources(root, crate):
            rel = _relative(path, root)
            in_impl = False
            for lineno, line in enumerate(
                path.read_text(encoding="utf-8", errors="replace").splitlines(), 1
            ):
                if _IMPL.match(line):
                    in_impl = type_name in line
                    continue
                if in_impl and _PUB_MUT_SELF.search(line):
                    findings.append(
                        Finding(
                            rel,
                            lineno,
                            RULE_STATEFUL,
                            line.strip(),
                            f"{crate}: stateful service method on {type_name}",
                        )
                    )
                if in_impl and _CLOSE.match(line):
                    in_impl = False
    return findings


def scan(root: Path = ROOT) -> list[Finding]:
    """Every D9 violation in the tree under ``root``."""
    return scan_globals(root) + scan_services(root)


def self_test() -> int:
    """Assert each rule still matches its bad fixture and not its near-miss.

    A pattern that silently stops matching is worse than no guard, so the
    guard runs this on every invocation: the broken sample must trip every
    rule and the clean sample must trip none.
    """
    if not TREE.is_dir():
        print(f"ERROR: stateless self-test fixtures missing at {TREE}")
        return 1
    findings = scan(TREE)
    problems = fixture_problems(findings, SELF_TEST_GROUPS)
    by_path = group_findings(findings)
    required_total = sum(len(group.rules) for group in SELF_TEST_GROUPS)
    good_total = sum(
        len(by_path.get(good, [])) for group in SELF_TEST_GROUPS for good in group.good
    )
    if problems:
        report_self_test_failure("ERROR: stateless self-test failed", problems)
        return 1
    print(
        "stateless guard: self-test ok "
        f"({required_total} required rule flag(s) matched their bad fixture, "
        f"{good_total} finding(s) in the good fixtures)"
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
            f"D9 violation [{finding.message}]: "
            f"{finding.path}:{finding.line}: {finding.anchor}"
        )
    if findings:
        print("")
        print("ERROR: forbidden process-global state in the bottom core (D9).")
        print("Move the state into an explicit value owned by the middle layer.")
        return 1
    print(
        "D9 guard: clean "
        "(no globals/caches, no stateful services in the bottom core)"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
