//! r3manager's core, without GPUI: the AWS CLI's profiles ([`profiles`]) and the S3 operations a developer runs on
//! the buckets and objects of the radosgw account behind one of them ([`s3`]).

pub mod profiles;
pub mod s3;
