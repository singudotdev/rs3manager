# 1. S3 through aws-sdk-s3, on its own tokio thread

Date: 2026-11-03. Status: accepted.

## Context

r3manager lets developers manage the buckets and objects of the radosgw account behind each of their AWS CLI
profiles. The client needs SigV4, path-style addressing (the gateway has no wildcard DNS), ListObjectsV2 pagination with
a delimiter, multipart upload, copy and metadata, and it reads the profiles from the AWS CLI's `~/.aws` files,
`endpoint_url` included. GPUI runs its own executor, not tokio.

The options were [aws-sdk-s3](https://docs.rs/aws-sdk-s3), the official SDK, which needs tokio and is heavy to build;
[rust-s3](https://docs.rs/rust-s3), lighter and enough for this CRUD, but community-maintained with a smaller API; and
our own SigV4 client, with signing, streaming and multipart to write and test by hand.

## Decision

- [aws-sdk-s3](https://docs.rs/aws-sdk-s3) with
  [force_path_style](https://docs.rs/aws-sdk-s3/latest/aws_sdk_s3/config/struct.Builder.html#method.force_path_style)
  always on, and [aws-config](https://docs.rs/aws-config) to load the profiles, whose `endpoint_url` the Rust SDK
  [supports](https://docs.aws.amazon.com/sdkref/latest/guide/feature-ss-endpoints.html).
- One thread owns a tokio runtime and the clients. The app sends it commands over a channel and gets results back the
  same way, so tokio stays at that boundary and never runs inside GPUI's async code.

## Consequences

- The first build is slow and the dependency tree is large.
- Every later S3 feature (versions, bucket settings, presigned URLs) is an SDK call, with no client work.
- A profile set up in the app works with `aws --profile`, and the other way round.
