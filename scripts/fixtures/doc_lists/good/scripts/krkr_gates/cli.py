"""Fixture registry: the gate's ordered steps.

Not the real registry — the self-test scans this file the way the check scans
`scripts/krkr_gates/cli.py`, with a `docs/` tree beside it that the pages
below describe.
"""

GATE_SEQUENCE: list[tuple[str, str, str, list[str]]] = [
    ("cargo fmt", "cargo", "cargo", ["cargo", "fmt", "--all", "--", "--check"]),
    ("stateless", "check", "stateless", []),
    ("error-handling", "check", "error-handling", []),
    ("native-parity", "check", "native-parity", ["--no-doc"]),
]
