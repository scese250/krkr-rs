"""Log-format guard (docs/rules/logging.md).

Every log record the engine writes starts with a subsystem: the crate and the
module inside it, so a reader can tell where a line came from without reading
the source.  The vocabulary is in `docs/rules/logging.md` section 1 and it is
the whole list: a subsystem that is not in it is added there.

The sites that predate the convention are in
`scripts/allowlists/log_format_baseline.txt`, keyed to the format literal
rather than to a line number, so a line shift does not make an entry stale and
a rewritten record does.  A site that is added must comply.
"""

import json
import re
import sys
from pathlib import Path

from ..common import ALLOWLISTS_DIR, FIXTURES_DIR, REPO_ROOT
from ..framework import Finding, filter_findings, print_all_findings

ROOT = REPO_ROOT
BASELINE = ALLOWLISTS_DIR / "log_format_baseline.txt"

CRATES = ROOT / "crates"
FIXTURES = FIXTURES_DIR / "log_format"

#: The subsystems a record may name, from `docs/rules/logging.md` section 1.
SUBSYSTEMS = (
    "app.audio",
    "app.frame",
    "app.runner",
    "app.startup",
    "android.host",
    "android.input",
    "compat.audio",
    "compat.event",
    "compat.font",
    "compat.image",
    "compat.kag",
    "compat.layer",
    "compat.menu",
    "compat.plugin",
    "compat.psb",
    "compat.scripts",
    "compat.sound",
    "compat.storage",
    "compat.system",
    "compat.timer",
    "engine.audio",
    "engine.frame",
    "engine.jobpool",
    "engine.projection",
    "engine.scene",
    "engine.storage",
    "engine.text",
    "engine.timer",
    "platform.fs",
    "platform.linux",
    "platform.media",
    "platform.render",
    "vm.tjs",
)

#: The log macros whose first argument is the record.
LOG_MACRO = re.compile(r"\blog::(info|warn|error|debug|trace)!\s*\(")

#: A subsystem followed by the colon that ends it.
SUBSYSTEM_RE = re.compile(r"^([a-z][a-z0-9_]*(?:\.[a-z][a-z0-9_]*)*): ")

RULE = "log-subsystem"

BAD_FIXTURE = "bad_log.rs"
GOOD_FIXTURE = "good_log.rs"


def read_literal(src: str, start: int):
    """The text of the string literal that starts at `start`.

    `None` when the argument is not a literal.  A raw literal (`r"..."` and
    `r#"..."#`) is read as it stands.  The text is the record itself, without
    the quotes, so the check reads what the reader reads.
    """
    if src.startswith("r", start):
        probe = start + 1
        hashes = 0
        while probe < len(src) and src[probe] == "#":
            hashes += 1
            probe += 1
        if probe < len(src) and src[probe] == '"':
            end = src.find('"' + "#" * hashes, probe + 1)
            if end != -1:
                return src[probe + 1 : end]
    if start >= len(src) or src[start] != '"':
        return None
    index = start + 1
    while index < len(src):
        char = src[index]
        if char == "\\":
            index += 2
            continue
        if char == '"':
            return src[start + 1 : index]
        index += 1
    return None


def record_sites(src: str):
    """The (offset, record) of every log call site in `src`.

    The record is the format literal, or the empty string when the argument is
    not one: a message held in a variable cannot be checked, so the site is a
    finding until it carries a literal.
    """
    sites = []
    for match in LOG_MACRO.finditer(src):
        start = match.end()
        while start < len(src) and src[start].isspace():
            start += 1
        literal = read_literal(src, start)
        sites.append((match.start(), "" if literal is None else literal))
    return sites


def is_compliant(record: str) -> bool:
    """Whether a record names one of the subsystems and nothing else."""
    if not record:
        return False
    match = SUBSYSTEM_RE.match(record)
    if match is None:
        return False
    return match.group(1) in SUBSYSTEMS


def findings_for(path: Path) -> list:
    """The findings of one file."""
    try:
        src = path.read_text(encoding="utf-8")
    except OSError:
        return []
    findings = []
    for offset, record in record_sites(src):
        if is_compliant(record):
            continue
        line = src.count("\n", 0, offset) + 1
        findings.append(
            Finding(
                rule=RULE,
                path=str(path.relative_to(ROOT)),
                line=line,
                anchor=record if record else "<not a literal>",
            )
        )
    return findings


def scan(root: Path = CRATES) -> list:
    """Every finding under `root`."""
    findings = []
    for path in sorted(root.rglob("*.rs")):
        findings.extend(findings_for(path))
    return findings


def load_baseline() -> list:
    """The ``(path, anchor, reason)`` baseline entries."""
    if not BASELINE.exists():
        return []
    entries = []
    for raw in BASELINE.read_text(encoding="utf-8").splitlines():
        text = raw.strip()
        if not text or text.startswith("#"):
            continue
        try:
            obj = json.loads(text)
        except json.JSONDecodeError:
            continue
        path = str(obj.get("path", ""))
        anchor = str(obj.get("anchor", ""))
        reason = str(obj.get("reason", ""))
        if path and anchor:
            entries.append((path, anchor, reason))
    return entries


def baseline_matches(entry, finding: Finding) -> bool:
    """Whether a baseline entry suppresses a finding."""
    return entry[0] == finding.path and entry[1] == finding.anchor


def self_test() -> int:
    """The bad fixture must trip the rule; the good fixture must trip nothing."""
    if not FIXTURES.is_dir():
        print(f"ERROR: log-format self-test fixtures missing at {FIXTURES}")
        return 1
    findings = scan(FIXTURES)
    bad = [f for f in findings if Path(f.path).name == BAD_FIXTURE]
    good = [f for f in findings if Path(f.path).name == GOOD_FIXTURE]
    if not bad:
        print(
            f"ERROR: log-format self-test: {BAD_FIXTURE} matched no `{RULE}` "
            "finding. Fix the rule; do not silence it."
        )
        return 1
    if good:
        print(
            f"ERROR: log-format self-test: {GOOD_FIXTURE} unexpectedly matched "
            f"{good[0].anchor}; a record with a subsystem must not be flagged."
        )
        return 1
    return 0


def main(argv=None) -> int:
    args = set(argv or [])
    selftest = self_test()
    if selftest != 0:
        return selftest
    if "--self-test" in args:
        return 0

    findings = scan()
    if "--print-all" in args:
        print_all_findings(findings)
        return 0

    baseline = load_baseline()
    unallowed, used = filter_findings(
        findings, baseline, baseline_matches, first_match=True
    )

    for finding in unallowed:
        print(
            f"ERROR: log-format violation [{finding.rule}] "
            f"{finding.path}:{finding.line}: {finding.anchor}"
        )

    if unallowed:
        print("")
        print(
            f"ERROR: {len(unallowed)} log record(s) do not name a subsystem "
            f"({len(findings)} site(s) checked, {len(baseline)} baselined)."
        )
        print("  Every record starts with its subsystem (docs/rules/logging.md")
        print("  section 1), or add a baseline entry to")
        print(f"  {BASELINE.relative_to(ROOT)}")
        return 1

    print(
        f"log-format guard: clean ({len(findings)} site(s) checked, "
        f"{len(baseline)} baselined) — {BASELINE.relative_to(ROOT)}"
    )
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
