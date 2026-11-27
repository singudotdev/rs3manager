#!/usr/bin/env bash
# Runs the core's integration tests over HTTPS: a TLS proxy with a throwaway certificate (dev/tls-proxy.py)
# fronts the local Ceph, and the tests trust that certificate (its CA) through SSL_CERT_FILE, which the SDK's HTTPS client reads.
# The shared-links test is left out: it fetches the link with its own plain-HTTP client. Checks that the client works with TLS, and that it refuses a certificate it wasn't given. Extra arguments go to the
# test binary.
set -euo pipefail
cd "$(dirname "$0")/.."
dev/rgw.sh up >/dev/null
work=$(mktemp -d)
proxy=""
trap '[ -n "$proxy" ] && kill "$proxy" 2>/dev/null; rm -rf "$work"' EXIT
# A throwaway CA and a server certificate it signs for 127.0.0.1: rustls refuses a CA certificate used as the server's.
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -subj "/CN=rs3manager test CA" \
  -keyout "$work/ca-key.pem" -out "$work/ca.pem" 2>/dev/null
openssl req -newkey rsa:2048 -nodes -subj "/CN=127.0.0.1" -keyout "$work/key.pem" -out "$work/server.csr" 2>/dev/null
printf 'subjectAltName=IP:127.0.0.1\nbasicConstraints=CA:FALSE\nextendedKeyUsage=serverAuth\n' >"$work/ext.cnf"
openssl x509 -req -in "$work/server.csr" -CA "$work/ca.pem" -CAkey "$work/ca-key.pem" -CAcreateserial -days 1 \
  -extfile "$work/ext.cnf" -out "$work/cert.pem" 2>/dev/null
port=${TLS_PORT:-7443}
python3 dev/tls-proxy.py "$port" "$work/cert.pem" "$work/key.pem" >"$work/proxy.log" 2>&1 &
proxy=$!
until grep -q '^https' "$work/proxy.log" 2>/dev/null; do kill -0 "$proxy" 2>/dev/null || { cat "$work/proxy.log"; exit 1; }; sleep 0.2; done

profile=$(dev/rgw.sh account rs3manager-test)
export R3M_TEST_ENDPOINT="https://127.0.0.1:$port"
R3M_TEST_ACCESS_KEY=$(sed -n 's/^aws_access_key_id = //p' <<<"$profile")
R3M_TEST_SECRET_KEY=$(sed -n 's/^aws_secret_access_key = //p' <<<"$profile")
export R3M_TEST_ACCESS_KEY R3M_TEST_SECRET_KEY
# The project accounts and users the policy tests use, as dev/test-rgw.sh sets them up (their keys are the gateway's,
# whatever the address they connect to).
keys() { # PREFIX < profile: exports PREFIX_ACCESS_KEY and PREFIX_SECRET_KEY
  local profile
  profile=$(cat)
  export "$1_ACCESS_KEY=$(sed -n 's/^aws_access_key_id = //p' <<<"$profile")"
  export "$1_SECRET_KEY=$(sed -n 's/^aws_secret_access_key = //p' <<<"$profile")"
}
keys R3M_TEST_PROD_ROOT < <(dev/rgw.sh account r3mtest-prod)
keys R3M_TEST_PROD_RO < <(dev/rgw.sh project r3mtest prod tester ro)
keys R3M_TEST_DEV_RW < <(dev/rgw.sh project r3mtest dev tester rw)

echo "== trusting the certificate: the operations work over TLS"
SSL_CERT_FILE="$work/ca.pem" cargo test -p rs3manager-core --test rgw -- --ignored --skip shared_links "$@"

echo "== not trusting it: the client refuses to connect"
if cargo test -p rs3manager-core --test rgw -- --ignored buckets_are_created_listed_and_deleted >"$work/untrusted.log" 2>&1; then
  echo "FAIL: the client talked to a gateway whose certificate it doesn't trust"
  exit 1
fi
echo "ok: refused"
