#!/usr/bin/env python3
"""Check the names the Android package resolves survive the shrinker.

The release APK job ends with `scripts/apk_surface_check.sh`, which takes the
built package apart and fails when a name the engine resolves -- by JNI field,
by entry point -- is no longer in it. That check needs the APK, so it runs
after a nine-minute build, and the only builds that run it are the ones a
`v*` tag starts.

This is the compile-free half of the same contract, so the gate catches the
drift before a tag is spent:

* every static field name `crates/android/src/jni_fields.rs` declares is
  named in `android/app/proguard-rules.pro`, or the shrinker may rename it
  and the engine reads a field that is not there;
* the engine Activity the keep rule names is the one the manifest starts, or
  the rule keeps a class the package never loads;
* the release script reads no name it does not define. It runs nowhere but a
  tag build, so a name the tree no longer defines is a failure that costs a
  tag to find.

The field names are read through `scripts/apk_surface_check.py`'s own reader,
which is their one owner: a second copy of the list here is a table that can
drift from the report that ships.

Usage:
    uv run --project scripts krkr-gate apk-surface
    uv run --project scripts krkr-gate apk-surface --self-test
    uv run --project scripts krkr-gate apk-surface --quiet

Exit status is non-zero on any finding.
"""

from __future__ import annotations

import argparse
import ast
import builtins
import re
import sys
from pathlib import Path

from ..common import FIXTURES_DIR, REPO_ROOT

KEEP_RULES_SOURCE = "android/app/proguard-rules.pro"
SCRIPT_SOURCE = "scripts/apk_surface_check.py"
MANIFEST_SOURCE = "android/app/src/main/AndroidManifest.xml"
# The Activity class the launcher starts is the one carrying the field names.
ENGINE_ACTIVITY = "dev.krkr.rs.KrkrGameActivity"

KEEP_CLASS_RE = re.compile(r"^-keep\s+class\s+([\w.$]+)\s*\{", re.MULTILINE)
ACTIVITY_RE = re.compile(r"<activity\b[^>]*>")
NAME_ATTR_RE = re.compile(r'android:name\s*=\s*"([^"]+)"')
PACKAGE_RE = re.compile(r'<manifest\b[^>]*\bpackage\s*=\s*"([^"]+)"')
# The application id a source manifest without a package resolves against.
GRADLE_SOURCE = "android/app/build.gradle.kts"
NAMESPACE_RE = re.compile(r'^\s*namespace\s*=\s*"([^"]+)"', re.MULTILINE)

Finding = tuple[str, str]


def _apk_surface_check():
    """The module that owns how the field names are read."""
    scripts = str(Path(__file__).resolve().parent.parent.parent)
    if scripts not in sys.path:
        sys.path.insert(0, scripts)
    import apk_surface_check

    return apk_surface_check


def undefined_names(source: str) -> list[str]:
    """The names 'source' reads that nothing in it binds.

    A name bound anywhere in the module counts as bound, which keeps the check
    free of false findings; what it catches is the name that is defined
    nowhere, which is what a deletion leaves behind.
    """
    tree = ast.parse(source)
    bound = set(dir(builtins)) | {"__file__", "__name__", "__doc__", "__spec__"}
    for node in ast.walk(tree):
        if isinstance(node, ast.Name) and not isinstance(node.ctx, ast.Load):
            bound.add(node.id)
        elif isinstance(node, ast.arg):
            bound.add(node.arg)
        elif isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef, ast.ClassDef)):
            bound.add(node.name)
        elif isinstance(node, ast.alias):
            bound.add((node.asname or node.name).split(".")[0])
        elif isinstance(node, ast.ExceptHandler) and node.name:
            bound.add(node.name)
        elif isinstance(node, (ast.Global, ast.Nonlocal)):
            bound.update(node.names)
    used = {
        node.id
        for node in ast.walk(tree)
        if isinstance(node, ast.Name) and isinstance(node.ctx, ast.Load)
    }
    return sorted(used - bound)


def check_script(root: Path) -> list[Finding]:
    """Validate the release script under 'root'."""
    path = root / SCRIPT_SOURCE
    try:
        source = path.read_text(encoding="utf-8")
    except OSError as error:
        return [("script", f"cannot read {SCRIPT_SOURCE}: {error}")]
    missing = undefined_names(source)
    if not missing:
        return []
    return [
        (
            "script-names",
            f"{SCRIPT_SOURCE} reads {', '.join(missing)}, which nothing in it binds; "
            f"the release job is the only place it runs, so it would fail a tag build",
        )
    ]


def check_tree(root: Path) -> list[Finding]:
    """Validate the tree under 'root'."""
    report = _apk_surface_check()
    findings: list[Finding] = []

    try:
        fields, _signature = report.read_jni_field_names(root)
    except report.CheckError as error:
        return [("jni-fields", str(error))]

    rules_path = root / KEEP_RULES_SOURCE
    try:
        parsed = report.parse_keep_rules(rules_path)
        rules = rules_path.read_text(encoding="utf-8")
    except report.CheckError as error:
        return [("keep-rules", str(error))]
    except OSError as error:
        return [("keep-rules", f"cannot read {KEEP_RULES_SOURCE}: {error}")]

    # A field is kept when a rule for the Activity names it as a static field.
    # Searching the file's text would accept a name that only a comment
    # mentions, which keeps nothing.
    kept_fields = {
        name
        for rule in parsed
        if rule.name == ENGINE_ACTIVITY
        for kind, _type, name in rule.members
        if kind == "static-field"
    }
    for _constant, field_name in sorted(fields.items()):
        if field_name not in kept_fields:
            findings.append(
                (
                    "keep-rules-name-field",
                    f"{KEEP_RULES_SOURCE} keeps no static field {field_name} on "
                    f"{ENGINE_ACTIVITY}; the native side reads it with "
                    f"GetStaticFieldID, so the shrinker may rename it",
                )
            )

    manifest_path = root / MANIFEST_SOURCE
    try:
        manifest = manifest_path.read_text(encoding="utf-8")
    except OSError as error:
        return findings + [
            ("manifest", f"cannot read {MANIFEST_SOURCE}: {error}")
        ]

    # A source manifest names an activity relatively against its package, and
    # the keep rules name the resolved class.
    package_match = PACKAGE_RE.search(manifest)
    package = package_match.group(1) if package_match else None
    if package is None:
        # A module manifest carries no package: the build resolves its
        # relative activity names against the Gradle namespace.
        gradle_path = root / GRADLE_SOURCE
        try:
            gradle = gradle_path.read_text(encoding="utf-8")
        except OSError as error:
            return findings + [
                ("manifest", f"cannot read {GRADLE_SOURCE}: {error}")
            ]
        namespace_match = NAMESPACE_RE.search(gradle)
        package = namespace_match.group(1) if namespace_match else None
    if package is None:
        return findings + [
            (
                "manifest",
                f"{MANIFEST_SOURCE} names an activity relatively and neither it nor "
                f"{GRADLE_SOURCE} declares a package",
            )
        ]
    started: set[str] = set()
    for element in ACTIVITY_RE.findall(manifest):
        name_match = NAME_ATTR_RE.search(element)
        if not name_match:
            continue
        try:
            started.add(report.qualify(package, name_match.group(1)))
        except report.CheckError as error:
            return findings + [("manifest", str(error))]
    if ENGINE_ACTIVITY not in started:
        findings.append(
            (
                "manifest-activity",
                f"{MANIFEST_SOURCE} starts no {ENGINE_ACTIVITY}; the keep rule and the "
                f"static fields belong to the class the manifest names",
            )
        )
    kept = {rule.name for rule in parsed}
    for name in sorted(kept):
        if name.startswith("dev.krkr.rs.") and name not in started:
            findings.append(
                (
                    "keep-rule-class",
                    f"{KEEP_RULES_SOURCE} keeps {name}, which {MANIFEST_SOURCE} does "
                    f"not start and nothing else resolves by name",
                )
            )
    return findings


def self_test(verbose: bool = True) -> int:
    """Prove the rules fire on the bad tree and stay quiet on the good one."""
    good = FIXTURES_DIR / "apk_surface" / "good"
    bad = FIXTURES_DIR / "apk_surface" / "bad"
    for tree in (good, bad):
        if not tree.is_dir():
            print(f"check-apk-surface: self-test fixture missing: {tree}")
            return 1

    problems = check_tree(good)
    if problems:
        print("check-apk-surface: self-test FAILED: the good fixture reports findings")
        for rule, message in problems:
            print(f"  {rule}: {message}")
        return 1

    fired = {rule for rule, _message in check_tree(bad)}
    expected = {"keep-rules-name-field", "manifest-activity", "keep-rule-class"}
    if fired != expected:
        print("check-apk-surface: self-test FAILED: the bad fixture fired the wrong rules")
        print(f"  expected: {sorted(expected)}")
        print(f"  fired:    {sorted(fired)}")
        return 1

    # The name check is what keeps a deletion in the release script from
    # reaching a tag build, so the self-test proves it fires.
    if undefined_names("def f():\n    return NOT_DEFINED\n") != ["NOT_DEFINED"]:
        print("check-apk-surface: self-test FAILED: the name check fires on nothing")
        return 1
    script_problems = check_script(REPO_ROOT)
    if script_problems:
        print(f"check-apk-surface: self-test FAILED: {SCRIPT_SOURCE} does not resolve")
        for rule, message in script_problems:
            print(f"  {rule}: {message}")
        return 1

    if verbose:
        print(
            f"check-apk-surface: self-test ok ({len(expected)} rule(s) fired, "
            f"{SCRIPT_SOURCE} resolves)"
        )
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--self-test", action="store_true", help="run the fixture self-test and exit"
    )
    parser.add_argument("--quiet", action="store_true", help="only print failures")
    parser.add_argument(
        "--repo-root", default=str(REPO_ROOT), help="the tree to check"
    )
    args = parser.parse_args(argv)

    if args.self_test:
        return self_test()

    root = Path(args.repo_root)
    findings = check_tree(root) + check_script(root)
    if not findings:
        if not args.quiet:
            print(
                "check-apk-surface: clean (the Android keep rules name every JNI field, "
                "keep only classes the manifest starts, and the release script resolves)"
            )
        return 0
    for rule, message in findings:
        print(f"check-apk-surface: {rule}: {message}")
    return 1


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
