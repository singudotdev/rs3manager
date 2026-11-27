//! Buckets, folder listings, object details and headers, rename and delete: every operation but transfers.
//!
//! Folders are S3's: a common prefix ending in `/` when listing with that delimiter, plus the empty marker object the
//! console tools (`MinIO`'s, the AWS console) create for an empty folder, which rs3manager creates too.

use super::error::map;
use super::names::{check_bucket_name, check_key, copy_source_version};
use super::{Bucket, DeleteReport, Headers, ObjectEntry, ObjectInfo, Page, Pending, Preview, S3Error, Session};
use super::{size, time};
use aws_sdk_s3::Client;
use aws_sdk_s3::presigning::PresigningConfig;
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart, Delete, MetadataDirective, ObjectIdentifier};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;
use tokio::task::JoinSet;

/// The largest object `CopyObject` copies in one request; larger ones are copied in parts.
const COPY_LIMIT: u64 = 5 * 1024 * 1024 * 1024;

/// Most objects one `DeleteObjects` request takes.
const DELETE_BATCH: usize = 1000;

/// Sets every field of [`Headers`] on a request builder that has them (`PutObject`, `CopyObject`,
/// `CreateMultipartUpload`): one place that knows the list, so no request forgets one.
macro_rules! with_headers {
    ($builder:expr, $headers:expr) => {{
        let h: &Headers = $headers;
        $builder
            .set_content_type(h.content_type.clone())
            .set_cache_control(h.cache_control.clone())
            .set_content_disposition(h.content_disposition.clone())
            .set_content_encoding(h.content_encoding.clone())
            .set_content_language(h.content_language.clone())
            .set_metadata(Some(h.user.iter().map(|(k, v)| (k.clone(), v.clone())).collect()))
    }};
}
pub(crate) use with_headers;

impl Session {
    /// The account's buckets, by name. With `check_access`, those the profile may not list are found by trying (one
    /// listing each); without, all are taken as listable, for a caller that knows the policies.
    pub fn list_buckets(&self, check_access: bool) -> Pending<Vec<Bucket>> {
        self.spawn(move |client| async move {
            let mut buckets = Vec::new();
            let mut token = None;
            loop {
                let out = client.list_buckets().set_continuation_token(token).send().await.map_err(map)?;
                buckets.extend(out.buckets().iter().map(|b| Bucket {
                    name: b.name().unwrap_or_default().to_owned(),
                    created: time(b.creation_date()),
                    listable: true,
                }));
                token = out.continuation_token().map(str::to_owned);
                if token.is_none() {
                    break;
                }
            }
            buckets.sort_by(|a, b| a.name.cmp(&b.name));
            if check_access {
                mark_unlistable(&client, &mut buckets).await;
            }
            Ok(buckets)
        })
    }

    /// Creates bucket `name`, checked against S3's naming rules first. A name already taken is refused even when the
    /// account owns it, which radosgw (like AWS in us-east-1) would otherwise answer with success. radosgw answers the
    /// check with 404 for a name nobody has, before looking at policies, so a 403 means it exists out of the profile's
    /// reach; a name the policies don't allow is refused by the create itself.
    pub fn create_bucket(&self, name: &str) -> Pending<()> {
        let name = name.to_owned();
        self.spawn(|client| async move {
            check_bucket_name(&name)?;
            match client.head_bucket().bucket(&name).send().await.map_err(map) {
                Err(S3Error::NotFound(_)) => {}
                Ok(_) | Err(S3Error::AccessDenied(_)) => return Err(S3Error::BucketExists),
                Err(e) => return Err(e),
            }
            client.create_bucket().bucket(&name).send().await.map_err(map)?;
            Ok(())
        })
    }

    /// Deletes bucket `name`. With `force`, first aborts its unfinished uploads and deletes every object, version and
    /// delete marker in it; without, a bucket with anything in it is refused ([`S3Error::BucketNotEmpty`]).
    pub fn delete_bucket(&self, name: &str, force: bool) -> Pending<()> {
        let name = name.to_owned();
        self.spawn(|client| async move {
            if force {
                abort_uploads(&client, &name).await?;
                empty_bucket(&client, &name).await?;
            }
            client.delete_bucket().bucket(&name).send().await.map_err(map)?;
            Ok(())
        })
    }

    /// One page of folder `prefix` (empty for the bucket's top, else ending in `/`) of `bucket`, from `token`.
    pub fn list(&self, bucket: &str, prefix: &str, token: Option<String>) -> Pending<Page> {
        let (bucket, prefix) = (bucket.to_owned(), prefix.to_owned());
        self.spawn(|client| async move {
            let out = client
                .list_objects_v2()
                .bucket(&bucket)
                .prefix(&prefix)
                .delimiter("/")
                .set_continuation_token(token)
                .send()
                .await
                .map_err(map)?;
            Ok(Page {
                folders: out.common_prefixes().iter().filter_map(|p| p.prefix().map(str::to_owned)).collect(),
                objects: out
                    .contents()
                    .iter()
                    .filter_map(|o| {
                        let key = o.key()?;
                        (key != prefix).then(|| ObjectEntry {
                            key: key.to_owned(),
                            size: size(o.size()),
                            modified: time(o.last_modified()),
                        })
                    })
                    .collect(),
                next: out.next_continuation_token().map(str::to_owned),
            })
        })
    }

    /// Object `key`'s details and headers.
    pub fn head(&self, bucket: &str, key: &str) -> Pending<ObjectInfo> {
        let (bucket, key) = (bucket.to_owned(), key.to_owned());
        self.spawn(|client| async move { head(&client, &bucket, &key).await })
    }

    /// Up to `limit` bytes from the start of object `key`, to show it.
    pub fn preview(&self, bucket: &str, key: &str, limit: u64) -> Pending<Preview> {
        let (bucket, key) = (bucket.to_owned(), key.to_owned());
        self.spawn(|client| async move {
            let range = format!("bytes=0-{}", limit.saturating_sub(1));
            let out = client.get_object().bucket(&bucket).key(&key).range(range).send().await.map_err(map)?;
            // The total is after the `/` of `Content-Range: bytes 0-N/TOTAL`; no range header means the whole object.
            let total = out
                .content_range()
                .and_then(|r| r.rsplit_once('/'))
                .and_then(|(_, t)| t.parse::<u64>().ok())
                .unwrap_or_else(|| size(out.content_length()));
            let bytes = out
                .body
                .collect()
                .await
                .map_err(|e| S3Error::Network(format!("reading {key}: {e}")))?
                .into_bytes()
                .to_vec();
            let truncated = total > bytes.len() as u64;
            Ok(Preview { bytes, truncated })
        })
    }

    /// A link that downloads object `key` without credentials until `expires` from now, signed with the profile's
    /// keys: anyone holding it can read the object meanwhile, and it can't be revoked short of changing those keys.
    /// The signature works offline, so the object isn't checked to exist. `SigV4` caps `expires` at seven days, and a
    /// link signed with temporary credentials (SSO) stops working when they expire.
    pub fn share(&self, bucket: &str, key: &str, expires: Duration) -> Pending<String> {
        let (bucket, key) = (bucket.to_owned(), key.to_owned());
        self.spawn(move |client| async move {
            let config =
                PresigningConfig::expires_in(expires).map_err(|e| S3Error::Invalid(format!("link expiry: {e}")))?;
            let request = client.get_object().bucket(&bucket).key(&key).presigned(config).await.map_err(map)?;
            Ok(request.uri().to_owned())
        })
    }

    /// Replaces object `key`'s headers and user metadata with `headers`, copying the object onto itself as S3 requires.
    pub fn set_headers(&self, bucket: &str, key: &str, headers: Headers) -> Pending<ObjectInfo> {
        let (bucket, key) = (bucket.to_owned(), key.to_owned());
        self.spawn(|client| async move {
            headers.check()?;
            copy(&client, &bucket, (&key, None), &key, Some(&headers)).await?;
            head(&client, &bucket, &key).await
        })
    }

    /// Creates an empty folder `prefix` (ending in `/`) with the marker object folder tools use.
    pub fn create_folder(&self, bucket: &str, prefix: &str) -> Pending<()> {
        let (bucket, prefix) = (bucket.to_owned(), prefix.to_owned());
        self.spawn(|client| async move {
            check_key(&prefix)?;
            if !prefix.ends_with('/') {
                return Err(S3Error::Invalid(format!("folder {prefix:?}: it must end in '/'")));
            }
            client.put_object().bucket(&bucket).key(&prefix).send().await.map_err(map)?;
            Ok(())
        })
    }

    /// Renames or moves object `from` to `to` in `bucket`, headers kept. Refuses when `to` exists.
    pub fn rename(&self, bucket: &str, from: &str, to: &str) -> Pending<()> {
        let (bucket, from, to) = (bucket.to_owned(), from.to_owned(), to.to_owned());
        self.spawn(|client| async move {
            check_key(&to)?;
            if from == to {
                return Ok(());
            }
            match head(&client, &bucket, &to).await {
                Ok(_) => return Err(S3Error::ObjectExists(to)),
                Err(S3Error::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
            copy(&client, &bucket, (&from, None), &to, None).await?;
            client.delete_object().bucket(&bucket).key(&from).send().await.map_err(map)?;
            Ok(())
        })
    }

    /// Renames or moves folder `from` to `to` (both ending in `/`), every object under it, returning how many moved.
    /// Refuses when anything is under `to` already, or when `to` is inside `from`.
    pub fn rename_folder(&self, bucket: &str, from: &str, to: &str) -> Pending<u64> {
        let (bucket, from, to) = (bucket.to_owned(), from.to_owned(), to.to_owned());
        self.spawn(|client| async move {
            check_key(&to)?;
            if !from.ends_with('/') || !to.ends_with('/') {
                return Err(S3Error::Invalid("folder names must end in '/'".to_owned()));
            }
            if to.starts_with(&from) {
                return Err(S3Error::Invalid(format!("can't move {from:?} into itself")));
            }
            let occupied =
                client.list_objects_v2().bucket(&bucket).prefix(&to).max_keys(1).send().await.map_err(map)?;
            if !occupied.contents().is_empty() {
                return Err(S3Error::ObjectExists(to));
            }
            let mut moved = 0;
            let mut token = None;
            loop {
                let page = client
                    .list_objects_v2()
                    .bucket(&bucket)
                    .prefix(&from)
                    .set_continuation_token(token)
                    .send()
                    .await
                    .map_err(map)?;
                for key in page.contents().iter().filter_map(|o| o.key()) {
                    let target = format!("{to}{}", &key[from.len()..]);
                    check_key(&target)?;
                    copy(&client, &bucket, (key, None), &target, None).await?;
                    client.delete_object().bucket(&bucket).key(key).send().await.map_err(map)?;
                    moved += 1;
                }
                token = page.next_continuation_token().map(str::to_owned);
                if token.is_none() {
                    return Ok(moved);
                }
            }
        })
    }

    /// Deletes objects `keys` of `bucket`, in batches, reporting the ones the gateway refused.
    pub fn delete(&self, bucket: &str, keys: Vec<String>) -> Pending<DeleteReport> {
        let bucket = bucket.to_owned();
        self.spawn(|client| async move {
            let mut report = DeleteReport::default();
            for batch in keys.chunks(DELETE_BATCH) {
                delete_batch(&client, &bucket, batch.iter().map(|k| (k.as_str(), None)), &mut report).await?;
            }
            Ok(report)
        })
    }

    /// Deletes folder `prefix` (ending in `/`) of `bucket`: every object under it, its marker included.
    pub fn delete_folder(&self, bucket: &str, prefix: &str) -> Pending<DeleteReport> {
        let (bucket, prefix) = (bucket.to_owned(), prefix.to_owned());
        self.spawn(|client| async move {
            if !prefix.ends_with('/') {
                return Err(S3Error::Invalid(format!("folder {prefix:?}: it must end in '/'")));
            }
            delete_prefix(&client, &bucket, &prefix).await
        })
    }
}

/// Object `key`'s details.
pub(crate) async fn head(client: &Client, bucket: &str, key: &str) -> Result<ObjectInfo, S3Error> {
    head_version(client, bucket, key, None).await
}

/// Object `key`'s details, of `version` when given, else of the current version.
pub(crate) async fn head_version(
    client: &Client,
    bucket: &str,
    key: &str,
    version: Option<&str>,
) -> Result<ObjectInfo, S3Error> {
    let request = client.head_object().bucket(bucket).key(key).set_version_id(version.map(str::to_owned));
    let out = request.send().await.map_err(map)?;
    Ok(ObjectInfo {
        key: key.to_owned(),
        size: size(out.content_length()),
        modified: time(out.last_modified()),
        etag: out.e_tag().map(|e| e.trim_matches('"').to_owned()),
        version_id: out.version_id().filter(|v| *v != "null").map(str::to_owned),
        storage_class: out.storage_class().map(|c| c.as_str().to_owned()),
        headers: Headers {
            content_type: out.content_type().map(str::to_owned),
            cache_control: out.cache_control().map(str::to_owned),
            content_disposition: out.content_disposition().map(str::to_owned),
            content_encoding: out.content_encoding().and_then(content_encoding),
            content_language: out.content_language().map(str::to_owned),
            user: out
                .metadata()
                .map_or_else(Default::default, |m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect()),
        },
    })
}

/// An object's real `Content-Encoding` from what the gateway stored: without `aws-chunked`, the upload's transfer
/// encoding, which [S3 removes](https://docs.aws.amazon.com/AmazonS3/latest/API/sigv4-streaming.html) but radosgw
/// 19.2 keeps when a client (the AWS CLI's `s3 cp`, say) streams with it. Kept, it would be saved back as the object's.
fn content_encoding(stored: &str) -> Option<String> {
    let real: Vec<&str> = stored.split(',').map(str::trim).filter(|e| !e.is_empty() && *e != "aws-chunked").collect();
    (!real.is_empty()).then(|| real.join(", "))
}

/// Copies object `from` (a key, and a version of it when given, else the current one) to `to` within `bucket`: with
/// `headers`, replacing its headers and metadata, else keeping them. Objects over [`COPY_LIMIT`] are copied in
/// parts, server side.
pub(crate) async fn copy(
    client: &Client,
    bucket: &str,
    from: (&str, Option<&str>),
    to: &str,
    headers: Option<&Headers>,
) -> Result<(), S3Error> {
    let source = head_version(client, bucket, from.0, from.1).await?;
    let from = copy_source_version(bucket, from.0, from.1);
    if source.size <= COPY_LIMIT {
        let request = client.copy_object().bucket(bucket).key(to).copy_source(&from);
        let request = match headers {
            Some(h) => with_headers!(request.metadata_directive(MetadataDirective::Replace), h),
            None => request.metadata_directive(MetadataDirective::Copy),
        };
        request.send().await.map_err(map)?;
        return Ok(());
    }
    let headers = headers.unwrap_or(&source.headers);
    let created =
        with_headers!(client.create_multipart_upload().bucket(bucket).key(to), headers).send().await.map_err(map)?;
    let upload_id = created.upload_id().unwrap_or_default().to_owned();
    let result = copy_parts(client, bucket, &from, to, &upload_id, source.size).await;
    if result.is_err() {
        let _ = client.abort_multipart_upload().bucket(bucket).key(to).upload_id(&upload_id).send().await;
    }
    result
}

/// The parts of a multipart copy of `from` (an `x-amz-copy-source`, `len` bytes) into upload `upload_id` of `to`,
/// then its completion.
async fn copy_parts(
    client: &Client,
    bucket: &str,
    from: &str,
    to: &str,
    upload_id: &str,
    len: u64,
) -> Result<(), S3Error> {
    let part = super::transfer::part_size(len).max(512 * 1024 * 1024);
    let mut parts = Vec::new();
    let mut start = 0;
    let mut number = 1;
    while start < len {
        let end = (start + part).min(len) - 1;
        let out = client
            .upload_part_copy()
            .bucket(bucket)
            .key(to)
            .upload_id(upload_id)
            .part_number(number)
            .copy_source(from)
            .copy_source_range(format!("bytes={start}-{end}"))
            .send()
            .await
            .map_err(map)?;
        let etag = out.copy_part_result().and_then(|r| r.e_tag()).map(str::to_owned);
        parts.push(CompletedPart::builder().part_number(number).set_e_tag(etag).build());
        start = end + 1;
        number += 1;
    }
    client
        .complete_multipart_upload()
        .bucket(bucket)
        .key(to)
        .upload_id(upload_id)
        .multipart_upload(CompletedMultipartUpload::builder().set_parts(Some(parts)).build())
        .send()
        .await
        .map_err(map)?;
    Ok(())
}

/// Deletes `objects` (keys with an optional version) in one request, adding to `report`.
async fn delete_batch<'a>(
    client: &Client,
    bucket: &str,
    objects: impl Iterator<Item = (&'a str, Option<&'a str>)>,
    report: &mut DeleteReport,
) -> Result<(), S3Error> {
    let ids = objects
        .map(|(key, version)| ObjectIdentifier::builder().key(key).set_version_id(version.map(str::to_owned)).build())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| S3Error::Internal(e.to_string()))?;
    if ids.is_empty() {
        return Ok(());
    }
    let asked = ids.len() as u64;
    let delete =
        Delete::builder().set_objects(Some(ids)).quiet(true).build().map_err(|e| S3Error::Internal(e.to_string()))?;
    let out = client.delete_objects().bucket(bucket).delete(delete).send().await.map_err(map)?;
    let failed = out.errors();
    report.deleted += asked - failed.len() as u64;
    report.failed.extend(failed.iter().map(|e| {
        let why = e.message().or(e.code()).unwrap_or("refused");
        (e.key().unwrap_or_default().to_owned(), why.to_owned())
    }));
    Ok(())
}

/// Deletes every object under `prefix` of `bucket`, a page at a time.
async fn delete_prefix(client: &Client, bucket: &str, prefix: &str) -> Result<DeleteReport, S3Error> {
    let mut report = DeleteReport::default();
    let mut token = None;
    loop {
        let page = client
            .list_objects_v2()
            .bucket(bucket)
            .prefix(prefix)
            .set_continuation_token(token)
            .send()
            .await
            .map_err(map)?;
        delete_batch(client, bucket, page.contents().iter().filter_map(|o| o.key()).map(|k| (k, None)), &mut report)
            .await?;
        token = page.next_continuation_token().map(str::to_owned);
        if token.is_none() {
            return Ok(report);
        }
    }
}

/// Deletes every version and delete marker in `bucket`, or every object when the gateway can't list versions.
async fn empty_bucket(client: &Client, bucket: &str) -> Result<(), S3Error> {
    let (mut key_marker, mut version_marker) = (None, None);
    loop {
        let page = match client
            .list_object_versions()
            .bucket(bucket)
            .set_key_marker(key_marker)
            .set_version_id_marker(version_marker)
            .send()
            .await
            .map_err(map)
        {
            Ok(page) => page,
            Err(S3Error::Service { .. }) => return delete_prefix(client, bucket, "").await.map(drop),
            Err(e) => return Err(e),
        };
        let versions = page.versions().iter().filter_map(|v| Some((v.key()?, v.version_id())));
        let markers = page.delete_markers().iter().filter_map(|m| Some((m.key()?, m.version_id())));
        let all: Vec<_> = versions.chain(markers).collect();
        let mut report = DeleteReport::default();
        for batch in all.chunks(DELETE_BATCH) {
            delete_batch(client, bucket, batch.iter().copied(), &mut report).await?;
        }
        if let Some((key, why)) = report.failed.into_iter().next() {
            return Err(S3Error::Service { code: "DeleteFailed".to_owned(), message: format!("{key}: {why}") });
        }
        if !page.is_truncated().unwrap_or(false) {
            return Ok(());
        }
        key_marker = page.next_key_marker().map(str::to_owned);
        version_marker = page.next_version_id_marker().map(str::to_owned);
    }
}

/// Aborts every unfinished multipart upload in `bucket`, which would otherwise keep it from being deleted.
async fn abort_uploads(client: &Client, bucket: &str) -> Result<(), S3Error> {
    let (mut key_marker, mut id_marker) = (None, None);
    loop {
        let page = client
            .list_multipart_uploads()
            .bucket(bucket)
            .set_key_marker(key_marker)
            .set_upload_id_marker(id_marker)
            .send()
            .await
            .map_err(map)?;
        for upload in page.uploads() {
            if let (Some(key), Some(id)) = (upload.key(), upload.upload_id()) {
                client.abort_multipart_upload().bucket(bucket).key(key).upload_id(id).send().await.map_err(map)?;
            }
        }
        if !page.is_truncated().unwrap_or(false) {
            return Ok(());
        }
        key_marker = page.next_key_marker().map(str::to_owned);
        id_marker = page.next_upload_id_marker().map(str::to_owned);
    }
}

/// How many buckets are checked at once by [`mark_unlistable`].
const ACCESS_CHECKS: usize = 8;

/// Marks the buckets the profile may not list. `ListBuckets` shows an account user every bucket of the account,
/// including those their policies keep them out of; one listing of a single key per bucket tells them apart. Any
/// answer but a denial leaves the bucket listable, so a slow or failing gateway never hides one.
async fn mark_unlistable(client: &Client, buckets: &mut [Bucket]) {
    let limit = Arc::new(Semaphore::new(ACCESS_CHECKS));
    let mut checks = JoinSet::new();
    for (i, bucket) in buckets.iter().enumerate() {
        let (client, name, limit) = (client.clone(), bucket.name.clone(), Arc::clone(&limit));
        checks.spawn(async move {
            let _permit = limit.acquire_owned().await;
            let denied = matches!(
                client.list_objects_v2().bucket(&name).max_keys(1).send().await.map_err(map),
                Err(S3Error::AccessDenied(_))
            );
            (i, denied)
        });
    }
    while let Some(joined) = checks.join_next().await {
        if let Ok((i, true)) = joined {
            buckets[i].listable = false;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_upload_transfer_encoding_is_not_the_objects() {
        assert_eq!(content_encoding("aws-chunked"), None);
        assert_eq!(content_encoding("aws-chunked, gzip").as_deref(), Some("gzip"));
        assert_eq!(content_encoding("gzip").as_deref(), Some("gzip"));
    }
}
