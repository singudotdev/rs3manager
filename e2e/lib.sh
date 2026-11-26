# shellcheck shell=bash
# What the e2e scripts share, sourced by them (it runs nothing itself).

# Stops the script, saying why.
fail() {
    echo "FAIL: $*" >&2
    exit 1
}
