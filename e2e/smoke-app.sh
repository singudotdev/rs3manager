#!/usr/bin/env bash
# Smoke check of the app as built for release, on Linux: it starts on a sandboxed home with one profile (a gateway
# nobody answers at, so it shows what it does when it can't connect), stays up, idles (no work, no animation: it
# redraws only when something changes), and a screenshot of the screen is saved for a person to look at. With no
# display it fails saying why.
#
# Usage: e2e/smoke-app.sh [APP] [OUT]: APP is the app's binary (target/release/rs3manager), OUT the folder the
# screenshot goes to (target/smoke). It needs a display: CI runs it under `xvfb-run`, with Mesa's software Vulkan, and
# ImageMagick's `import` for the screenshot.
set -euo pipefail

repo=$(cd "$(dirname "$0")/.." && pwd)
# shellcheck source=e2e/lib.sh
. "$(dirname "$0")/lib.sh"
target=${CARGO_TARGET_DIR:-$repo/target}
app=${1:-$target/release/rs3manager}
out=${2:-$target/smoke}
# Percent of one core the idle app may use, averaged over the measure.
idle_limit=3
work=$(mktemp -d "${TMPDIR:-/tmp}/rs3-smoke.XXXXXX")
pid=""
cleanup() {
    if [ -n "$pid" ]; then kill "$pid" 2>/dev/null || true; fi
    rm -rf -- "$work"
}
trap cleanup EXIT

# The AWS CLI's files and what the app remembers go under the sandboxed home, never the user's own.
export HOME="$work/home"
unset XDG_STATE_HOME XDG_CONFIG_HOME XDG_DATA_HOME AWS_CONFIG_FILE AWS_SHARED_CREDENTIALS_FILE AWS_PROFILE
mkdir -p "$HOME/.aws" "$out"
cat >"$HOME/.aws/config" <<'CONFIG'
[profile smoke]
region = us-east-1
endpoint_url = http://127.0.0.1:9
CONFIG
cat >"$HOME/.aws/credentials" <<'CREDENTIALS'
[smoke]
aws_access_key_id = SMOKEACCESSKEY
aws_secret_access_key = smokesecretkey
CREDENTIALS

# With no display at all, the app fails within the timeout saying WHY, rather than running headless, unseen, forever,
# or panicking.
status=0
env -u DISPLAY -u WAYLAND_DISPLAY -u WAYLAND_SOCKET timeout 30 "$app" >"$work/no-display.log" 2>&1 || status=$?
[ "$status" -eq 1 ] || fail "with no display the app exited with $status, not 1: $(cat "$work/no-display.log")"
grep -qF 'run rs3manager from a desktop session' "$work/no-display.log" ||
    fail "with no display the app didn't say why: $(cat "$work/no-display.log")"

"$app" >"$work/app.log" 2>&1 &
pid=$!
# Its first frames, fonts loaded, and the profile's first request refused.
sleep 15
kill -0 "$pid" 2>/dev/null || fail "the app exited: $(cat "$work/app.log")"
if grep -q 'panicked' "$work/app.log"; then fail "the app panicked: $(cat "$work/app.log")"; fi

# CPU time the app's process used, in hundredths of a second: utime and stime, fields 14 and 15 of /proc/PID/stat;
# after the `(name)`, which may hold spaces, they're the 12th and 13th.
cpu() {
    local ticks
    ticks=$(getconf CLK_TCK)
    sed 's/.*) //' "/proc/$pid/stat" | awk -v t="$ticks" '{ print int(($12 + $13) * 100 / t) }'
}
before=$(cpu)
sleep 20
after=$(cpu)
used=$(((after - before) / 20))
echo "idle: ${used}% of a core"
[ "$used" -le "$idle_limit" ] || fail "the idle app used ${used}% of a core, over ${idle_limit}%"

if command -v import >/dev/null; then
    import -window root "$out/app-linux.png"
else
    echo "no ImageMagick import: no screenshot"
fi
kill -0 "$pid" 2>/dev/null || fail "the app exited while idle: $(cat "$work/app.log")"
echo "PASS smoke-app.sh"
