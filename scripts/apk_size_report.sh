#!/usr/bin/env bash
# Report the composition of the release APK, and verify the surface the engine
# resolves by name.
#
# The package is the artifact that reaches a device, so two things about it
# are settled here:
#
#   * **what it is made of**, measured: every entry, grouped the way the
#     package is built (each `.dex`, each native library, `resources.arsc`,
#     the `res/` tree, the `META-INF/` and `kotlin/` metadata), with the
#     totals, so that a later build can be compared with this one;
#   * **whether the names the native code resolves are still in it**: the
#     static fields the engine reads off its Activity through JNI, and the
#     entry points the game-activity glue resolves in the native library. A
#     shrinker that renames or drops one of them leaves a package that
#     installs and then dies on the device, with nothing in this log to show
#     for it.
#
# The self-test runs first: it takes the package apart, breaks one of those
# names on purpose, and refuses to accept the result. A check that cannot
# fail is not a check.
#
# Usage:
#   scripts/apk_size_report.sh [<path to the .apk>]
#
# With no path the release package under `android/app/build/outputs/apk` is
# used. The report goes to the job log, and to the run summary when the
# runner provides one.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
helper="$repo_root/scripts/apk_size_report.py"

die() {
    echo "apk-size-report: $*" >&2
    exit 1
}

apk=""
while [ $# -gt 0 ]; do
    case "$1" in
        -h|--help)
            sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            apk="$1"
            shift
            ;;
    esac
done

if [ -z "$apk" ]; then
    apk="$(find "$repo_root/android/app/build/outputs/apk/release" -maxdepth 1 \
        -name '*.apk' -print -quit 2>/dev/null || true)"
fi
[ -n "$apk" ] || die "no APK to report on; the release package was not assembled"
[ -f "$apk" ] || die "$apk is not a file"

report="$(mktemp)"
trap 'rm -f "$report"' EXIT

# The self-test comes first: it proves the checks can still fail before the
# package is measured with them.
echo "apk-size-report: self-test"
python3 "$helper" --apk "$apk" --repo-root "$repo_root" --self-test | tee -a "$report"
echo ""

# The report itself. The exit status of the helper is the status of the step:
# a package that lost one of the names, or that grew past the budget, fails
# the job here rather than on a device.
set +e
python3 "$helper" --apk "$apk" --repo-root "$repo_root" | tee -a "$report"
status=${PIPESTATUS[0]}
set -e

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    {
        echo "## APK composition"
        echo
        echo '```'
        cat "$report"
        echo '```'
    } >> "$GITHUB_STEP_SUMMARY"
fi

exit "$status"
