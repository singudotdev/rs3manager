#!/usr/bin/env bash
# A radosgw to develop r3manager against, and the identities developers connect with.
#
#   dev/rgw.sh up                 start a local radosgw (podman), data kept in a volume
#   dev/rgw.sh user NAME          local: create an S3 user, print its AWS CLI profile
#   dev/rgw.sh down               stop it, keeping the data
#   dev/rgw.sh reset              stop it and delete the data
#   dev/rgw.sh logs               follow its log
#   dev/rgw.sh account NAME URL   on a Ceph node: create an account and its root user, print the profile for URL
#
# The local radosgw is the real one (the image Ceph releases) with the SQLite `dbstore` backend instead of RADOS, so
# it needs no monitors or OSDs. dbstore doesn't support accounts (`account create` fails with "Operation not
# supported"), so locally `user` makes a plain RGW user: for the buckets and objects r3manager works on, its keys
# behave as an account root user's do. `account` is for a real cluster, where radosgw-admin runs on the node.
set -euo pipefail

IMAGE=${RGW_IMAGE:-quay.io/ceph/ceph:v19}
NAME=${RGW_CONTAINER:-r3manager-rgw}
VOLUME=${RGW_VOLUME:-r3manager-rgw-data}
PORT=${RGW_PORT:-7480}
DATA=/var/lib/ceph/radosgw
# What radosgw and radosgw-admin need to run on dbstore without a cluster; both must name the same store.
STORE=(--no-mon-config --rgw_backend_store=dbstore --rgw_config_store=dbstore
  --dbstore_db_dir="$DATA" --dbstore_config_uri="file:$DATA/config.db")

die() { printf 'rgw.sh: %s\n' "$*" >&2; exit 1; }

# The access and secret key of the first key in radosgw-admin's JSON for a user, on stdin, as two lines.
keys_of() { python3 -c 'import json,sys; k=json.load(sys.stdin)["keys"][0]; print(k["access_key"]); print(k["secret_key"])'; }

# Prints the AWS CLI profile for NAME at ENDPOINT with the keys on stdin, ready for ~/.aws/config and credentials.
print_profile() {
  local name=$1 endpoint=$2 access secret
  { read -r access; read -r secret; }
  cat <<EOF
# ~/.aws/config
[profile $name]
endpoint_url = $endpoint
region = default

# ~/.aws/credentials
[$name]
aws_access_key_id = $access
aws_secret_access_key = $secret
EOF
}

up() {
  if podman container exists "$NAME"; then
    podman start "$NAME" >/dev/null
  else
    podman run -d --name "$NAME" -p "127.0.0.1:$PORT:7480" -v "$VOLUME:$DATA" --entrypoint radosgw "$IMAGE" \
      -d "${STORE[@]}" --rgw_frontends="beast port=7480" --rgw_data="$DATA" --run-dir=/var/run/ceph >/dev/null
  fi
  local _
  for _ in $(seq 1 30); do
    if curl -sf -o /dev/null "http://127.0.0.1:$PORT/"; then
      echo "radosgw up at http://127.0.0.1:$PORT"
      return
    fi
    sleep 1
  done
  die "radosgw didn't answer on port $PORT: see 'dev/rgw.sh logs'"
}

# Runs radosgw-admin in the local container, on its store, JSON on stdout and its chatter dropped.
local_admin() {
  podman container exists "$NAME" || die "not running: start it with 'dev/rgw.sh up'"
  podman exec "$NAME" radosgw-admin "${STORE[@]}" --format=json "$@" 2>/dev/null
}

user() {
  local uid=${1:?usage: dev/rgw.sh user NAME}
  local_admin user create --uid="$uid" --display-name="$uid" --gen-access-key --gen-secret >/dev/null \
    || die "couldn't create user '$uid' (does it exist already? 'radosgw-admin user info' shows its keys)"
  local_admin user info --uid="$uid" | keys_of | print_profile "$uid" "http://127.0.0.1:$PORT"
}

account() {
  local name=${1:?usage: dev/rgw.sh account NAME URL} endpoint=${2:?usage: dev/rgw.sh account NAME URL} id
  command -v radosgw-admin >/dev/null || die "radosgw-admin not found: run 'account' on a Ceph node"
  id=$(radosgw-admin account create --account-name="$name" --format=json \
    | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')
  radosgw-admin user create --uid="$name-root" --display-name="$name root" --account-id="$id" --account-root \
    --gen-access-key --gen-secret --format=json | keys_of | print_profile "$name" "$endpoint"
}

case ${1:-} in
  up) up ;;
  down) podman stop "$NAME" >/dev/null && podman rm "$NAME" >/dev/null ;;
  reset) podman rm -f "$NAME" >/dev/null; podman volume rm -f "$VOLUME" >/dev/null ;;
  logs) podman logs -f "$NAME" ;;
  user) shift; user "$@" ;;
  account) shift; account "$@" ;;
  *) sed -n '2,9s/^# \{0,1\}//p' "$0" >&2; exit 2 ;;
esac
