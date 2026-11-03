#!/usr/bin/env bash
# A Ceph cluster with radosgw to develop r3manager against, and the identities developers connect with.
#
#   dev/rgw.sh up                 start it (podman): one monitor, manager, OSD and radosgw; data kept in volumes
#   dev/rgw.sh user NAME          create an S3 user (or reuse it), print its AWS CLI profile
#   dev/rgw.sh account NAME       create a user account and its root user (or reuse them), print the root's profile
#   dev/rgw.sh down               stop it, keeping the data
#   dev/rgw.sh reset              stop it and delete the data
#   dev/rgw.sh logs               follow its log
#
# On a real cluster's node, where radosgw-admin runs directly: RGW_ADMIN=radosgw-admin RGW_ENDPOINT=URL dev/rgw.sh account NAME
#
# It's Ceph's own demo image (Squid), objects stored in RADOS on one OSD, so copies, multipart uploads, versions and
# accounts behave as on the production cluster: the standalone radosgw on dbstore got those wrong (see
# docs/decisions/0003-develop-against-a-one-node-ceph-in-a-container.md). It needs the host's network, as the monitor
# and OSD find each other by address; everything listens on 127.0.0.1.
set -euo pipefail

IMAGE=${RGW_IMAGE:-quay.io/ceph/demo:latest-squid}
NAME=${RGW_CONTAINER:-r3manager-ceph}
PORT=${RGW_PORT:-7480}
ENDPOINT=${RGW_ENDPOINT:-http://127.0.0.1:$PORT}

die() { printf 'rgw.sh: %s\n' "$*" >&2; exit 1; }

# radosgw-admin, JSON on stdout and its chatter dropped: in the container, or RGW_ADMIN on a cluster's node.
admin() {
  if [[ -n ${RGW_ADMIN:-} ]]; then
    "$RGW_ADMIN" --format=json "$@" 2>/dev/null
  else
    podman container exists "$NAME" || die "not running: start it with 'dev/rgw.sh up'"
    podman exec "$NAME" radosgw-admin --format=json "$@" 2>/dev/null
  fi
}

# The access and secret key of the first key in radosgw-admin's JSON for a user, on stdin, as two lines.
keys_of() { python3 -c 'import json,sys; k=json.load(sys.stdin)["keys"][0]; print(k["access_key"]); print(k["secret_key"])'; }

# Prints the AWS CLI profile NAME with the keys on stdin, ready for ~/.aws/config and ~/.aws/credentials.
print_profile() {
  local name=$1 access secret
  { read -r access; read -r secret; }
  cat <<EOF
# ~/.aws/config
[profile $name]
endpoint_url = $ENDPOINT
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
    # The demo insists on making a user of its own; `user` and `account` make the ones used. RGW_FRONTEND_IP keeps
    # radosgw off the LAN, which the host network would otherwise expose it to.
    podman run -d --name "$NAME" --net=host \
      -v "$NAME-etc:/etc/ceph" -v "$NAME-lib:/var/lib/ceph" \
      -e MON_IP=127.0.0.1 -e CEPH_PUBLIC_NETWORK=127.0.0.0/8 \
      -e RGW_FRONTEND_IP=127.0.0.1 -e RGW_FRONTEND_PORT="$PORT" -e CEPH_DEMO_UID=demo \
      -e DEMO_DAEMONS=osd,rgw -e OSD_TYPE=directory \
      "$IMAGE" demo >/dev/null
  fi
  local _
  for _ in $(seq 1 120); do
    if curl -sf -o /dev/null "http://127.0.0.1:$PORT/"; then
      echo "radosgw up at http://127.0.0.1:$PORT"
      return
    fi
    sleep 1
  done
  die "radosgw didn't answer on port $PORT in 2 minutes: see 'dev/rgw.sh logs'"
}

user() {
  local uid=${1:?usage: dev/rgw.sh user NAME}
  # Idempotent: an existing user's profile is printed again, so scripts and tests can call it every time.
  if ! admin user info --uid="$uid" >/dev/null; then
    admin user create --uid="$uid" --display-name="$uid" --gen-access-key --gen-secret >/dev/null \
      || die "couldn't create user '$uid'"
  fi
  admin user info --uid="$uid" | keys_of | print_profile "$uid"
}

account() {
  local name=${1:?usage: dev/rgw.sh account NAME} id uid
  uid="$name-root"
  if ! admin account get --account-name="$name" >/dev/null; then
    admin account create --account-name="$name" >/dev/null || die "couldn't create account '$name'"
  fi
  id=$(admin account get --account-name="$name" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')
  if ! admin user info --uid="$uid" >/dev/null; then
    admin user create --uid="$uid" --display-name="$uid" --account-id="$id" --account-root \
      --gen-access-key --gen-secret >/dev/null || die "couldn't create the root user of account '$name'"
  fi
  admin user info --uid="$uid" | keys_of | print_profile "$name"
}

case ${1:-} in
  up) up ;;
  down) podman stop "$NAME" >/dev/null ;;
  reset) podman rm -f "$NAME" >/dev/null; podman volume rm -f "$NAME-etc" "$NAME-lib" >/dev/null ;;
  logs) podman logs -f "$NAME" ;;
  user) shift; user "$@" ;;
  account) shift; account "$@" ;;
  *) sed -n '2,11s/^# \{0,1\}//p' "$0" >&2; exit 2 ;;
esac
