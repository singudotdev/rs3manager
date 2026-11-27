#!/usr/bin/env bash
# A Ceph cluster with radosgw to develop rs3manager against, and the identities developers connect with.
#
#   dev/rgw.sh up                 start it (podman): one monitor, manager, OSD and radosgw; data kept in volumes
#   dev/rgw.sh user NAME          create an S3 user (or reuse it), print its AWS CLI profile
#   dev/rgw.sh account NAME       create a user account and its root user (or reuse them) with the guardrails, print
#                                 the root's profile
#   dev/rgw.sh breakglass NAME    lift the guardrails of account NAME's root, through a throwaway second root; the next
#                                 `account NAME` puts them back (as they are in this script)
#   dev/rgw.sh admin ACCOUNT USER create devsecops admin USER in ACCOUNT (or reuse it), print its profile
#   dev/rgw.sh project PROJECT ENV USER rw|ro
#                                 account PROJECT-ENV with the team's groups, USER in read-write-objects and in
#                                 read-write-buckets (rw) or read-only-buckets (ro); print USER's profile. Done as
#                                 admin $PLATFORM_ADMIN (default devsecops), created if needed
#   dev/rgw.sh down               stop it, keeping the data
#   dev/rgw.sh reset              stop it and delete the data
#   dev/rgw.sh logs               follow its log
#
# On a real cluster's node, where radosgw-admin runs directly: RGW_ADMIN=radosgw-admin RGW_ENDPOINT=URL dev/rgw.sh account NAME
# IAM calls go through the API (with `aws` if installed, else the amazon/aws-cli image): as a throwaway root made with
# radosgw-admin to set the guardrails and create admins, as an admin for `project`; never as the root, which can't.
#
# It's Ceph's own demo image (Squid), objects stored in RADOS on one OSD, so copies, multipart uploads, versions and
# accounts behave as on the production cluster: the standalone radosgw on dbstore got those wrong (see
# docs/decisions/0003-develop-against-a-one-node-ceph-in-a-container.md). It needs the host's network, as the monitor
# and OSD find each other by address; everything listens on 127.0.0.1.
set -euo pipefail

IMAGE=${RGW_IMAGE:-quay.io/ceph/demo:latest-squid}
NAME=${RGW_CONTAINER:-rs3manager-ceph}
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
  local access secret
  { read -r access; read -r secret; } < <(admin user info --uid="$uid" | keys_of)
  guard_root "$access" "$secret" "$uid" "$name"
  printf '%s\n%s\n' "$access" "$secret" | print_profile "$name"
}

# What nobody in a project account may do, the root included: rights are given only through the policies of the
# account's users and groups, which the platform team writes (docs/decisions/0005-*). Bucket policies and ACLs would
# let anyone grant access, even public, outside them.
SHARING='"s3:PutBucketPolicy","s3:DeleteBucketPolicy","s3:PutBucketAcl","s3:PutObjectAcl","s3:PutObjectVersionAcl"'

# What the root may not do in IAM: any write. It reads (rs3manager shows it what it may do) and works in S3, but users,
# groups, policies and keys change only through the devsecops admins, so its keys can't hand out rights (#32). The
# prefixes of every write in radosgw 19.2's IAM (src/rgw/rgw_iam_policy.cc); it refuses a pattern matching none.
ROOT_IAM_WRITES='"iam:Create*","iam:Delete*","iam:Update*","iam:Put*","iam:Attach*","iam:Detach*","iam:Add*",
  "iam:Remove*","iam:Tag*","iam:Untag*","iam:Modify*"'

# The guardrails policy of the root of an account: Deny the sharing actions and every IAM write. The latter also keeps
# it from deleting this policy, so only another root lifts it (see `breakglass`).
guardrails() {
  printf '%s' '{"Version":"2012-10-17","Statement":[{"Effect":"Deny","Action":['"$SHARING"'],"Resource":"*"},
    {"Effect":"Deny","Action":['"$ROOT_IAM_WRITES"'],"Resource":"*"}]}' | tr -d '\n'
}

# Runs "$2 ACCESS SECRET ${@:3}" as a throwaway second root of account $1, removed afterwards whatever happened: a
# root isn't bound by another root's policies, so this is how the guardrails are set or lifted and admins created.
# Needs radosgw-admin, which only devsecops have on the cluster. Returns what the command returned.
as_temp_root() {
  local name=$1 id temp access secret status=0
  shift
  id=$(admin account get --account-name="$name" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])') \
    || die "no account '$name'"
  temp="$name-breakglass-$$"
  { read -r access; read -r secret; } < <(admin user create --uid="$temp" --display-name="$temp" --account-id="$id" \
    --account-root --gen-access-key --gen-secret | keys_of)
  # In a subshell, so a `die` in the command ends only it and the throwaway root is still removed.
  ("$1" "$access" "$secret" "${@:2}") || status=$?
  # A spare root without guardrails is what the guardrails are meant to avoid.
  admin user rm --uid="$temp" >/dev/null || die "couldn't remove the throwaway root $temp: remove it by hand"
  return $status
}

put_guardrails() { aws_as "$1" "$2" iam put-user-policy --user-name "$3" --policy-name guardrails \
  --policy-document "$(guardrails)" >/dev/null; }
drop_guardrails() { aws_as "$1" "$2" iam delete-user-policy --user-name "$3" --policy-name guardrails >/dev/null; }

# Puts the guardrails on root user $3 (keys $1 $2) of account $4, unless it has them as they are in this script: an
# account set up before a change gets the new ones.
guard_root() {
  local access=$1 secret=$2 uid=$3 name=$4 current
  current=$(aws_as "$access" "$secret" iam get-user-policy --user-name "$uid" --policy-name guardrails \
    --query PolicyDocument --output json 2>/dev/null) || current=null
  if python3 -c '
import json, sys, urllib.parse
doc = json.loads(sys.argv[1])
# The CLI hands the document back decoded as JSON, or as the string radosgw stored.
if isinstance(doc, str):
    doc = json.loads(urllib.parse.unquote(doc))
sys.exit(doc != json.loads(sys.argv[2]))' "$current" "$(guardrails)" 2>/dev/null; then
    return
  fi
  as_temp_root "$name" put_guardrails "$uid" || die "couldn't put the guardrails on the root of $name"
}

# Lifts the guardrails of account NAME's root, through a throwaway second root.
breakglass() {
  local name=${1:?usage: dev/rgw.sh breakglass NAME}
  as_temp_root "$name" drop_guardrails "$name-root" || die "couldn't lift the guardrails of $name-root"
  echo "guardrails lifted from $name-root; 'dev/rgw.sh account $name' puts them back"
}

# The AWS CLI as the user whose keys are $1 and $2, against the gateway; the rest are its arguments.
aws_as() {
  local access=$1 secret=$2
  shift 2
  if command -v aws >/dev/null; then
    AWS_ACCESS_KEY_ID=$access AWS_SECRET_ACCESS_KEY=$secret AWS_DEFAULT_REGION=default \
      aws --endpoint-url "$ENDPOINT" "$@"
  else
    podman run --rm --network host -e AWS_ACCESS_KEY_ID="$access" -e AWS_SECRET_ACCESS_KEY="$secret" \
      -e AWS_DEFAULT_REGION=default docker.io/amazon/aws-cli --endpoint-url "$ENDPOINT" "$@"
  fi
}
# The policy document of group $1 for buckets PROJECT-*-ENV ($2, $3). A draft of the team's: replace it with theirs to
# test against exactly what production enforces.
group_policy() {
  local group=$1 buckets="arn:aws:s3:::$2-*-$3"
  # Every group carries the guardrails too, so an Allow added later (a managed policy) can't grant them.
  local deny='{"Effect":"Deny","Action":['"$SHARING"'],"Resource":"*"}'
  local list='{"Effect":"Allow","Action":"s3:ListAllMyBuckets","Resource":"*"},
    {"Effect":"Allow","Action":["s3:ListBucket","s3:GetBucketLocation","s3:ListBucketMultipartUploads",
      "s3:GetBucketVersioning","s3:ListBucketVersions","s3:GetBucketTagging","s3:GetLifecycleConfiguration",
      "s3:GetBucketCORS"],
      "Resource":"'"$buckets"'"}'
  case $group in
    read-write-objects) printf '%s' '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["s3:GetObject",
      "s3:PutObject","s3:DeleteObject","s3:GetObjectVersion","s3:DeleteObjectVersion","s3:AbortMultipartUpload",
      "s3:ListMultipartUploadParts"],
      "Resource":"'"$buckets"'/*"},'"$deny"']}' ;;
    read-only-buckets) printf '%s' '{"Version":"2012-10-17","Statement":['"$list"','"$deny"']}' ;;
    read-write-buckets) printf '%s' '{"Version":"2012-10-17","Statement":['"$list"',
      {"Effect":"Allow","Action":["s3:CreateBucket","s3:DeleteBucket","s3:PutBucketTagging",
      "s3:PutLifecycleConfiguration","s3:PutBucketCORS","s3:PutBucketVersioning"],
      "Resource":"'"$buckets"'"},'"$deny"']}' ;;
  esac
}
# Prints the keys of IAM user $1, two lines, through the IAM command in the other arguments: its first key, the secret
# read back with radosgw-admin (IAM never shows it again), else a new key. Idempotent, so scripts can call it each time.
iam_user_keys() {
  local user=$1 key
  shift
  key=$("$@" list-access-keys --user-name "$user" --query 'AccessKeyMetadata[0].AccessKeyId' --output text)
  if [[ -z $key || $key == None ]]; then
    "$@" create-access-key --user-name "$user" >/dev/null || die "couldn't create a key for $user"
    key=$("$@" list-access-keys --user-name "$user" --query 'AccessKeyMetadata[0].AccessKeyId' --output text)
  fi
  admin user info --access-key="$key" | python3 -c '
import json, sys
key = sys.argv[1]
print(key); print(next(k["secret_key"] for k in json.load(sys.stdin)["keys"] if k["access_key"] == key))' "$key"
}

# The keys (two lines) in a profile on stdin.
profile_keys() { sed -n 's/^aws_access_key_id = //p; s/^aws_secret_access_key = //p'; }

# Creates devsecops admin $2 in account $1 (or reuses it) and prints its profile. Admins live under IAM path
# /platform/ and administer the account with their own keys; the root can't write IAM, so a throwaway root creates
# them, and the root's keys are kept for emergencies (docs/decisions/0005-*).
# Their policy lets them do everything but the sharing actions and touching the root or any admin: their policies,
# keys, groups or the users themselves, so no admin lifts a guardrail or takes another's place. The path covers admins
# added later without rewriting anyone's policy.
platform_admin() {
  local account=${1:?usage: dev/rgw.sh admin ACCOUNT USER} user=${2:?USER}
  account "$account" >/dev/null
  as_temp_root "$account" make_admin "$account" "$user"
}

# platform_admin's IAM calls, with the keys $1 $2 of a throwaway root of account $3; $4 is the admin.
make_admin() {
  local access=$1 secret=$2 account=$3 user=$4 id
  id=$(admin account get --account-name="$account" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')
  local iam=(aws_as "$access" "$secret" iam) arn="arn:aws:iam::$id:user"
  "${iam[@]}" create-user --user-name "$user" --path /platform/ >/dev/null 2>&1 || true
  [[ $("${iam[@]}" get-user --user-name "$user" --query User.Path --output text) == /platform/ ]] \
    || die "$user exists in $account outside /platform/: not making it an admin"
  "${iam[@]}" put-user-policy --user-name "$user" --policy-name platform-admin --policy-document \
    '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:*","s3:*"],"Resource":"*"},
      {"Effect":"Deny","Action":['"$SHARING"'],"Resource":"*"},
      {"Effect":"Deny","Action":["iam:PutUserPolicy","iam:DeleteUserPolicy","iam:AttachUserPolicy",
        "iam:DetachUserPolicy","iam:DeleteUser","iam:UpdateUser","iam:CreateAccessKey","iam:UpdateAccessKey",
        "iam:DeleteAccessKey","iam:AddUserToGroup","iam:RemoveUserFromGroup"],
       "Resource":["'"$arn/platform/*"'","'"$arn/$account-root"'"]}]}' >/dev/null \
    || die "couldn't give $user its admin policy in $account"
  iam_user_keys "$user" "${iam[@]}" | print_profile "$user-$account"
}

project() {
  local project=${1:?usage: dev/rgw.sh project PROJECT ENV USER rw|ro} env=${2:?ENV} user=${3:?USER} mode=${4:?rw|ro}
  [[ $mode == rw || $mode == ro ]] || die "the bucket access is rw or ro, not '$mode'"
  local account="$project-$env" access secret group
  { read -r access; read -r secret; } < <(platform_admin "$account" "${PLATFORM_ADMIN:-devsecops}" | profile_keys)
  local iam=(aws_as "$access" "$secret" iam)
  for group in read-write-objects read-only-buckets read-write-buckets; do
    "${iam[@]}" create-group --group-name "$group" >/dev/null 2>&1 || true
    "${iam[@]}" put-group-policy --group-name "$group" --policy-name "$group" \
      --policy-document "$(group_policy "$group" "$project" "$env" | tr -d '\n')" >/dev/null \
      || die "couldn't set the policy of group $group in $account"
  done
  "${iam[@]}" create-user --user-name "$user" >/dev/null 2>&1 || true
  local buckets=read-only-buckets other=read-write-buckets
  [[ $mode == rw ]] && { buckets=read-write-buckets; other=read-only-buckets; }
  for group in read-write-objects "$buckets"; do
    "${iam[@]}" add-user-to-group --user-name "$user" --group-name "$group" >/dev/null \
      || die "couldn't add $user to $group in $account"
  done
  # Switching rw and ro takes the user out of the other group, which would otherwise still grant its rights.
  "${iam[@]}" remove-user-from-group --user-name "$user" --group-name "$other" >/dev/null 2>&1 || true
  # Reading its own policies lets rs3manager show only what the user may do. radosgw refuses policy variables
  # (${aws:username}) in IAM resources, so the ARNs are written out per user.
  local id iam_arn
  id=$(admin account get --account-name="$account" | python3 -c 'import json,sys; print(json.load(sys.stdin)["id"])')
  iam_arn="arn:aws:iam::$id"
  "${iam[@]}" put-user-policy --user-name "$user" --policy-name read-own-policies --policy-document \
    '{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":["iam:GetUser","iam:ListGroupsForUser",
      "iam:ListUserPolicies","iam:GetUserPolicy","iam:ListAttachedUserPolicies"],"Resource":"'"$iam_arn:user/$user"'"},
      {"Effect":"Allow","Action":["iam:ListGroupPolicies","iam:GetGroupPolicy","iam:ListAttachedGroupPolicies"],
      "Resource":["'"$iam_arn:group/read-write-objects"'","'"$iam_arn:group/$buckets"'"]}]}' >/dev/null \
    || die "couldn't let $user read its own policies in $account"
  iam_user_keys "$user" "${iam[@]}" | print_profile "$user-$account"
}
case ${1:-} in
  up) up ;;
  down) podman stop "$NAME" >/dev/null ;;
  reset) podman rm -f "$NAME" >/dev/null; podman volume rm -f "$NAME-etc" "$NAME-lib" >/dev/null ;;
  logs) podman logs -f "$NAME" ;;
  user) shift; user "$@" ;;
  account) shift; account "$@" ;;
  project) shift; project "$@" ;;
  breakglass) shift; breakglass "$@" ;;
  admin) shift; platform_admin "$@" ;;
  *) sed -n '2,20s/^# \{0,1\}//p' "$0" >&2; exit 2 ;;
esac
