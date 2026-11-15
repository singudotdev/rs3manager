#!/usr/bin/env bash
# Checks the guardrails `dev/rgw.sh` puts in an account against the local Ceph: the root can't set bucket policies or
# ACLs, nor write IAM (so nor remove its guardrails), still reads IAM and does its normal work, and `breakglass` lifts
# them for `account` to put back; a devsecops admin sets up a project but can't set bucket policies nor touch the
# root's or another admin's guardrails or keys. Run by dev/test-rgw.sh; uses a throwaway account.
set -euo pipefail
cd "$(dirname "$0")/.."
name=r3mguard-test
ok() { printf 'guardrails: ok   %s\n' "$*"; }
fail() { printf 'guardrails: FAIL %s\n' "$*" >&2; exit 1; }
profile=$(dev/rgw.sh account "$name")
access=$(sed -n 's/^aws_access_key_id = //p' <<<"$profile")
secret=$(sed -n 's/^aws_secret_access_key = //p' <<<"$profile")
endpoint=$(sed -n 's/^endpoint_url = //p' <<<"$profile")
# The AWS CLI with keys $1 $2, quiet: only whether it worked.
as() {
  local key=$1 secret=$2
  shift 2
  podman run --rm --network host -e AWS_ACCESS_KEY_ID="$key" -e AWS_SECRET_ACCESS_KEY="$secret" \
    -e AWS_DEFAULT_REGION=default docker.io/amazon/aws-cli --endpoint-url "$endpoint" "$@" >/dev/null 2>&1
}
root() { as "$access" "$secret" "$@"; }
bucket="$name-$$"
public='{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::'$bucket'/*"}]}'
root s3api create-bucket --bucket "$bucket" && ok "the root creates buckets" || fail "the root can't create a bucket"
trap 'root s3api delete-bucket --bucket "$bucket" || true' EXIT
root s3api put-bucket-policy --bucket "$bucket" --policy "$public" || true
root s3api get-bucket-policy --bucket "$bucket" && fail "the root set a bucket policy" || ok "no bucket policy"
root s3api put-bucket-acl --bucket "$bucket" --acl public-read && fail "the root set a bucket ACL" || ok "no bucket ACL"
root iam delete-user-policy --user-name "$name-root" --policy-name guardrails && fail "the root removed its guardrails" \
  || ok "the root can't remove its guardrails"
# IAM only through the admins: the root reads it, but writes nothing (#32).
root iam list-users && ok "the root reads IAM" || fail "the root can't read IAM"
root iam create-user --user-name by-root && fail "the root created a user" || ok "the root can't create users"
dev/rgw.sh breakglass "$name" >/dev/null && ok "break glass lifts them" || fail "break glass"
root s3api put-bucket-policy --bucket "$bucket" --policy "$public" && ok "lifted: the root may set a policy" \
  || fail "still guarded after break glass"
root s3api delete-bucket-policy --bucket "$bucket" || fail "couldn't delete the policy set while lifted"
root iam create-user --user-name by-root && root iam delete-user --user-name by-root \
  && ok "lifted: the root may write IAM" || fail "the root can't write IAM after break glass"
dev/rgw.sh account "$name" >/dev/null
root s3api put-bucket-policy --bucket "$bucket" --policy "$public" && fail "account didn't put them back" \
  || ok "account puts them back"
root iam create-user --user-name by-root && fail "account didn't put back the IAM deny" \
  || ok "account puts back the IAM deny"
[[ -z $(podman exec "${RGW_CONTAINER:-r3manager-ceph}" radosgw-admin user list 2>/dev/null | grep breakglass || true) ]] \
  && ok "no break-glass root left" || fail "a break-glass root was left behind"

# Devsecops admins: ds-a sets up a project with its own keys, ds-b is another admin it must leave alone.
keys_of() { sed -n 's/^aws_access_key_id = //p; s/^aws_secret_access_key = //p' | tr '\n' ' '; }
read -r a_key a_secret <<<"$(dev/rgw.sh admin "$name" ds-a | keys_of)"
dev/rgw.sh admin "$name" ds-b >/dev/null
admin_a() { as "$a_key" "$a_secret" "$@"; }
PLATFORM_ADMIN=ds-a dev/rgw.sh project r3mguard test someone rw >/dev/null && ok "an admin sets up a project" \
  || fail "an admin couldn't set up a project"
policy='{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Action":"s3:ListAllMyBuckets","Resource":"*"}]}'
root iam put-user-policy --user-name ds-a --policy-name probe --policy-document "$policy" \
  && fail "the root changed an admin's policies" || ok "the root can't change users' policies"
# Refused by policy: the answer is AccessDenied, not another failure (a user with policies can't be deleted anyway).
denied_a() {
  local out
  # Captured first: under pipefail, piping the CLI (failing by design here) into grep would report its failure.
  out=$(podman run --rm --network host -e AWS_ACCESS_KEY_ID="$a_key" -e AWS_SECRET_ACCESS_KEY="$a_secret" \
    -e AWS_DEFAULT_REGION=default docker.io/amazon/aws-cli --endpoint-url "$endpoint" "$@" 2>&1) || true
  grep -q AccessDenied <<<"$out"
}
# radosgw denies S3 requests with an empty message the CLI can't parse, so the check is the effect: no policy set.
admin_a s3api put-bucket-policy --bucket "$bucket" --policy "$public" || true
root s3api get-bucket-policy --bucket "$bucket" && fail "an admin set a bucket policy" || ok "an admin: no bucket policy"
denied_a iam delete-user-policy --user-name "$name-root" --policy-name guardrails \
  && ok "an admin can't lift the root's guardrails" || fail "an admin lifted the root's guardrails"
denied_a iam delete-user-policy --user-name ds-a --policy-name platform-admin \
  && ok "an admin can't remove its own policy" || fail "an admin removed its own policy"
# The same actions on a project user work: what stops them on admins is the guardrail.
admin_a iam put-user-policy --user-name someone --policy-name probe --policy-document "$policy" \
  && admin_a iam delete-user-policy --user-name someone --policy-name probe \
  && ok "an admin changes a project user's policies" || fail "an admin can't change a project user's policies"
denied_a iam put-user-policy --user-name ds-b --policy-name probe --policy-document "$policy" \
  && ok "an admin can't change another admin's policies" || fail "an admin changed another admin's policies"
admin_a iam create-user --user-name plain && admin_a iam delete-user --user-name plain \
  && ok "an admin creates and deletes project users" || fail "an admin can't delete a project user"
denied_a iam create-access-key --user-name ds-b && ok "an admin can't make keys for another admin" \
  || fail "an admin made a key for another admin"
denied_a iam delete-user --user-name ds-b && ok "an admin can't delete another admin" \
  || fail "an admin deleted another admin"
