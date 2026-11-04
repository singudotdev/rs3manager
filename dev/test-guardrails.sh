#!/usr/bin/env bash
# Checks the guardrails `dev/rgw.sh account` puts on an account's root user against the local Ceph: the root can't set
# bucket policies or ACLs nor remove its guardrails, still does its normal work, and `breakglass` lifts them for
# `account` to put back. Run by dev/test-rgw.sh; uses a throwaway account.
set -euo pipefail
cd "$(dirname "$0")/.."
name=r3mguard-test
ok() { printf 'guardrails: ok   %s\n' "$*"; }
fail() { printf 'guardrails: FAIL %s\n' "$*" >&2; exit 1; }
profile=$(dev/rgw.sh account "$name")
access=$(sed -n 's/^aws_access_key_id = //p' <<<"$profile")
secret=$(sed -n 's/^aws_secret_access_key = //p' <<<"$profile")
endpoint=$(sed -n 's/^endpoint_url = //p' <<<"$profile")
root() {
  podman run --rm --network host -e AWS_ACCESS_KEY_ID="$access" -e AWS_SECRET_ACCESS_KEY="$secret" \
    -e AWS_DEFAULT_REGION=default docker.io/amazon/aws-cli --endpoint-url "$endpoint" "$@" >/dev/null 2>&1
}
bucket="$name-$$"
public='{"Version":"2012-10-17","Statement":[{"Effect":"Allow","Principal":"*","Action":"s3:GetObject","Resource":"arn:aws:s3:::'$bucket'/*"}]}'
root s3api create-bucket --bucket "$bucket" && ok "the root creates buckets" || fail "the root can't create a bucket"
trap 'root s3api delete-bucket --bucket "$bucket" || true' EXIT
root s3api put-bucket-policy --bucket "$bucket" --policy "$public" && fail "the root set a bucket policy" || ok "no bucket policy"
root s3api put-bucket-acl --bucket "$bucket" --acl public-read && fail "the root set a bucket ACL" || ok "no bucket ACL"
root iam delete-user-policy --user-name "$name-root" --policy-name guardrails && fail "the root removed its guardrails" \
  || ok "the root can't remove its guardrails"
dev/rgw.sh breakglass "$name" >/dev/null && ok "break glass lifts them" || fail "break glass"
root s3api put-bucket-policy --bucket "$bucket" --policy "$public" && ok "lifted: the root may set a policy" \
  || fail "still guarded after break glass"
root s3api delete-bucket-policy --bucket "$bucket" || fail "couldn't delete the policy set while lifted"
dev/rgw.sh account "$name" >/dev/null
root s3api put-bucket-policy --bucket "$bucket" --policy "$public" && fail "account didn't put them back" \
  || ok "account puts them back"
[[ -z $(podman exec "${RGW_CONTAINER:-r3manager-ceph}" radosgw-admin user list 2>/dev/null | grep breakglass || true) ]] \
  && ok "no break-glass root left" || fail "a break-glass root was left behind"
