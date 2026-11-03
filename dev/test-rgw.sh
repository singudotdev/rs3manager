#!/usr/bin/env bash
# Runs the core's integration tests (`#[ignore]`d in a plain `cargo test`) against the local Ceph: starts it, creates
# the test account once, and passes its root user's keys (what developers connect with) to the tests. Extra arguments
# go to the test binary.
set -euo pipefail
cd "$(dirname "$0")/.."
dev/rgw.sh up >/dev/null
profile=$(dev/rgw.sh account r3manager-test)
R3M_TEST_ENDPOINT=$(sed -n 's/^endpoint_url = //p' <<<"$profile")
R3M_TEST_ACCESS_KEY=$(sed -n 's/^aws_access_key_id = //p' <<<"$profile")
R3M_TEST_SECRET_KEY=$(sed -n 's/^aws_secret_access_key = //p' <<<"$profile")
export R3M_TEST_ENDPOINT R3M_TEST_ACCESS_KEY R3M_TEST_SECRET_KEY
exec cargo test -p r3manager-core --test rgw -- --ignored "$@"
