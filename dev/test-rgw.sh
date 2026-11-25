#!/usr/bin/env bash
# Runs the integration tests (`#[ignore]`d in a plain `cargo test`) against the local Ceph: the core's, then the app's
# window driven by clicks and keys. Starts it, creates the test account once, and passes its root user's keys (what
# developers connect with) to the tests. Extra arguments go to the test binaries.
set -euo pipefail
cd "$(dirname "$0")/.."
dev/rgw.sh up >/dev/null
profile=$(dev/rgw.sh account rs3manager-test)
R3M_TEST_ENDPOINT=$(sed -n 's/^endpoint_url = //p' <<<"$profile")
R3M_TEST_ACCESS_KEY=$(sed -n 's/^aws_access_key_id = //p' <<<"$profile")
R3M_TEST_SECRET_KEY=$(sed -n 's/^aws_secret_access_key = //p' <<<"$profile")
export R3M_TEST_ENDPOINT R3M_TEST_ACCESS_KEY R3M_TEST_SECRET_KEY
# A project's accounts as teams set them up: tester may only read buckets in prod and manage them in dev, objects in
# both, and only buckets named r3mtest-*-ENV.
keys() { # PREFIX < profile: exports PREFIX_ACCESS_KEY and PREFIX_SECRET_KEY
  local profile
  profile=$(cat)
  export "$1_ACCESS_KEY=$(sed -n 's/^aws_access_key_id = //p' <<<"$profile")"
  export "$1_SECRET_KEY=$(sed -n 's/^aws_secret_access_key = //p' <<<"$profile")"
}
keys R3M_TEST_PROD_ROOT < <(dev/rgw.sh account r3mtest-prod)
keys R3M_TEST_PROD_RO < <(dev/rgw.sh project r3mtest prod tester ro)
keys R3M_TEST_DEV_RW < <(dev/rgw.sh project r3mtest dev tester rw)
dev/test-guardrails.sh
cargo test -p rs3manager-core --test rgw -- --ignored "$@"
cargo test -p rs3manager-app -- --ignored "$@"
