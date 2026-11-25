# 0004: Read the user's own IAM policies to show only what they may do

## Context

Teams give each project and environment a radosgw account (`PROJECT-ENV`) whose IAM users work under group policies:
objects read-write, buckets read-only (production) or read-write, and only buckets named `PROJECT-*-ENV`. Account users
start with no permissions ([radosgw accounts](https://docs.ceph.com/en/squid/radosgw/account/)), `ListBuckets` shows
them every bucket of the account, and denials come as `AccessDenied` with no message. Without knowing the policies the
window can only find out by failing.

## Decision

At connect the app reads the profile's identity and policies through radosgw's IAM API
([supported actions](https://docs.ceph.com/en/squid/radosgw/iam/)) with `aws-sdk-iam`: `GetUser`, the user's inline
and attached policies, `ListGroupsForUser` and each group's. It evaluates them per AWS's
[policy evaluation logic](https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_policies_evaluation-logic.html)
for identity policies in one account (explicit Deny wins, else Allow, else denied; `Action`/`NotAction`,
`Resource`/`NotResource` with `*` and `?`). Statements with a `Condition` or policy variable make an answer unknown.
The window hides what's denied and keeps what's allowed or unknown; the gateway still decides every request. Refresh
reads the policies again.

Admins grant each user a `read-own-policies` policy on its own user ARN and its groups (radosgw rejects
`${aws:username}` in IAM resources, so ARNs are literal); `dev/rgw.sh project` writes it. The account's root user
(`GetUser` names it `arn:aws:iam::ACCOUNT:root`) may do everything in its account. Without the policy, or on a gateway
without IAM, the window falls back to asking the gateway (marking buckets it can't list with one listing each).

## Consequences

- One IAM round of a few requests per connect and refresh; no per-action probing.
- Rights come only from the policies of an account's users and groups: the team doesn't use bucket policies (they're
  blocked on the cluster), so nothing outside what rs3manager reads can grant a right, and the evaluation is complete
  but for `Condition`s. Were bucket policies ever allowed, a right granted only by one would be hidden.
- Only the S3 managed policies radosgw supports (`AmazonS3FullAccess`, `AmazonS3ReadOnlyAccess`) are known.
- radosgw 19.2 orders three user-policy answers in a way the SDK rejects; `core/src/s3/rgw_iam.rs` fixes them before
  parsing. New dependencies: `aws-sdk-iam`, and `serde_json`, `aws-smithy-types`, `bytes`, `http-body` (already in the
  tree).
