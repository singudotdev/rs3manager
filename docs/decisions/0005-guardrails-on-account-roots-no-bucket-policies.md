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

r3manager reads the root's own policies too: the root may do everything in its account but what they deny.

## Consequences

- The root still manages IAM: it can give a user it creates `s3:PutBucketPolicy` and that user can set a bucket
  policy (verified). The guardrails stop the root doing it itself and stop it by mistake; keeping root keys with the
  platform team is what stops it on purpose. Denying the root IAM writes would close it, at the cost of doing every
  IAM change through break glass.
- Changing the guardrails takes break glass, by design.
