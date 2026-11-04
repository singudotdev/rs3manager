# 0005: Guardrails on every account root: no bucket policies or ACLs

## Context

Rights in a project account are given only through the policies of its IAM users and groups, written by the platform
team (decision 0004). Bucket policies and ACLs would let anyone holding the right grant access, public access
included, outside those policies. Project people don't administer accounts, so the account root is limited too.

Verified on radosgw 19.2 (`quay.io/ceph/demo:latest-squid`), per [radosgw accounts](https://docs.ceph.com/en/squid/radosgw/account/)
("it is possible to add policy that denies their access explicitly"):

- By default the root sets a public bucket policy and a `public-read` ACL.
- An explicit Deny in a user policy put on the root (by name, `PROJECT-ENV-root`) applies to it.
- The root deletes that policy unless the Deny also covers `iam:PutUserPolicy`, `DeleteUserPolicy`,
  `AttachUserPolicy` and `DetachUserPolicy` on its own user ARN; with them it can't.
- `radosgw-admin` manages only managed policies (`user policy attach/detach`), not inline ones; a second root made
  with `radosgw-admin user create --account-root` isn't bound by the first's policies and can delete them.

## Decision

`dev/rgw.sh account` puts a `guardrails` policy on every root: Deny `s3:PutBucketPolicy`, `DeleteBucketPolicy`,
`PutBucketAcl`, `PutObjectAcl`, `PutObjectVersionAcl` on everything, and Deny changing its own policies. Every group
policy carries the same Deny, so an Allow added later (a managed policy) can't grant them. `dev/rgw.sh breakglass`
lifts the guardrails through a throwaway second root, removed right after; `account` puts them back.
`dev/test-guardrails.sh` checks all of it.

Devsecops administer each account through their own IAM users, one per person per account, under IAM path
`/platform/`: `dev/rgw.sh admin ACCOUNT USER` creates them with policy `platform-admin` (Allow `iam:*` and `s3:*`;
Deny the sharing actions; Deny changing the policies, keys, groups or the users themselves of the root and of
`user/platform/*`), and `dev/rgw.sh project` sets projects up as admin `$PLATFORM_ADMIN`. The root's keys are kept for
emergencies. Their admin users outside accounts can't do this: verified, every IAM call from a `--admin --system`
user with no account answers `MethodNotAllowed`, as IAM acts on the caller's account; and radosgw 19.2 has no
`AdministratorAccess` managed policy, so `platform-admin` is inline.

r3manager reads the root's own policies too: the root may do everything in its account but what they deny.

## Consequences

- Anyone with IAM writes, the root or an admin, can still create a user allowed `s3:PutBucketPolicy`, and that user
  can set a bucket policy (verified). radosgw's IAM has no permissions boundaries to prevent it, so the guardrails stop
  doing it directly or by mistake; only devsecops holding IAM-writing keys stops it on purpose.
- An admin protects every other admin by the path, so a new admin needs no change to the others' policies; an
  admin's own keys are rotated by the root or another route, as admins can't touch keys under `/platform/`.
- A person in devsecops has one IAM user and key per account; the script prints each as an AWS CLI profile.
- Changing the guardrails takes break glass, by design.
