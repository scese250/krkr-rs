#!/usr/bin/env bash
# Verify the surface the release APK resolves by name.
#
# The package is the artifact that reaches a device, so the names the engine
# looks up are settled here: the static fields it reads off its Activity
# through JNI, and the entry points the game-activity glue resolves in the
# native library. A shrinker that renames or drops one of them leaves a
# package that installs and then dies on the device, with nothing in the
# build log to show for it.
#
# The self-test runs first: it takes the package apart, breaks one of those
# names on purpose, and refuses to accept the result. A check that cannot
# fail is not a check.
#
# Usage:
#   scripts/apk_surface_check.sh [<path to the .apk>]
#
# With no path the release package under `android/app/build/outputs/apk` is
# used. The findings go to the job log, and to the run summary when the
# runner provides one.
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
helper="$repo_root/scripts/apk_surface_check.py"

die() {
    echo "apk-surface-check: $*" >&2
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
[ -n "$apk" ] || die "no APK to check; the release package was not assembled"
[ -f "$apk" ] || die "$apk is not a file"

report="$(mktemp)"
trap 'rm -f "$report"' EXIT

# The self-test comes first: it proves the checks can still fail before the
# package is measured with them.
echo "apk-surface-check: self-test"
python3 "$helper" --apk "$apk" --repo-root "$repo_root" --self-test | tee -a "$report"
echo ""

# The check itself. The exit status of the helper is the status of the step:
# a package that lost one of the names fails the job here rather than on a
# device.
set +e
python3 "$helper" --apk "$apk" --repo-root "$repo_root" | tee -a "$report"
status=${PIPESTATUS[0]}
set -e

if [ -n "${GITHUB_STEP_SUMMARY:-}" ]; then
    {
        echo "## APK surface"
        echo
        echo '```'
        cat "$report"
        echo '```'
    } >> "$GITHUB_STEP_SUMMARY"
fi

exit "$status"
