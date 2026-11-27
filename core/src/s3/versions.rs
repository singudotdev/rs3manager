//! An object's versions in a bucket that keeps them, and the bucket's versioning (decision #57): list them, restore
//! one by copying it over the key (it becomes the current version, the others kept, as
//! [Restoring previous versions](https://docs.aws.amazon.com/AmazonS3/latest/userguide/RestoringPreviousVersions.html)
//! describes), delete one for good.

use super::error::map;
use super::ops::{copy, head};
use super::{ObjectInfo, Pending, Session, size, time};
use aws_sdk_s3::types::{BucketVersioningStatus, VersioningConfiguration};
use std::time::SystemTime;

/// A version of an object, or a delete marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    /// Its ID: `null` for what was written before the bucket kept versions.
    pub id: String,
    /// Whether it's the current version.
    pub latest: bool,
    /// Whether it's a delete marker: the object reads as deleted while it's current.
    pub delete_marker: bool,
    /// When it was written.
    pub modified: Option<SystemTime>,
    /// Its size in bytes; none for a delete marker.
    pub size: u64,
    /// Its `ETag`, without quotes.
    pub etag: Option<String>,
}

/// Whether a bucket keeps versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Versioning {
    /// It never has.
    Off,
    /// Each write keeps the one before.
    Enabled,
    /// It did and no longer does; the versions kept stay.
    Suspended,
}

/// `versions` ordered as shown: the current one first, then newest first.
fn ordered(mut versions: Vec<Version>) -> Vec<Version> {
    versions.sort_by(|a, b| b.latest.cmp(&a.latest).then(b.modified.cmp(&a.modified)));
    versions
}

impl Session {
    /// Object `key`'s versions and delete markers in `bucket`, the current one first; a single `null` version when
    /// the bucket never kept versions.
    pub fn versions(&self, bucket: &str, key: &str) -> Pending<Vec<Version>> {
        let (bucket, key) = (bucket.to_owned(), key.to_owned());
        self.spawn(|client| async move {
            let mut found = Vec::new();
            let (mut key_marker, mut version_marker) = (None, None);
            loop {
                let page = client
                    .list_object_versions()
                    .bucket(&bucket)
                    .prefix(&key)
                    .set_key_marker(key_marker)
                    .set_version_id_marker(version_marker)
                    .send()
                    .await
                    .map_err(map)?;
                // Keys starting with `key` come after it, so a page that reaches one has every version of `key`.
                let mut past = false;
                for v in page.versions() {
                    if v.key() != Some(key.as_str()) {
                        past = true;
                        continue;
                    }
                    found.push(Version {
                        id: v.version_id().unwrap_or("null").to_owned(),
                        latest: v.is_latest() == Some(true),
                        delete_marker: false,
                        modified: time(v.last_modified()),
                        size: size(v.size()),
                        etag: v.e_tag().map(|e| e.trim_matches('"').to_owned()),
                    });
                }
                for m in page.delete_markers() {
                    if m.key() != Some(key.as_str()) {
                        past = true;
                        continue;
                    }
                    found.push(Version {
                        id: m.version_id().unwrap_or("null").to_owned(),
                        latest: m.is_latest() == Some(true),
                        delete_marker: true,
                        modified: time(m.last_modified()),
                        size: 0,
                        etag: None,
                    });
                }
                if past || page.is_truncated() != Some(true) {
                    return Ok(ordered(found));
                }
                key_marker = page.next_key_marker().map(str::to_owned);
                version_marker = page.next_version_id_marker().map(str::to_owned);
            }
        })
    }

    /// Makes version `id` of object `key` the current one again, by copying it over the key with its headers; the
    /// versions in between are kept. Returns the object as it now is.
    pub fn restore_version(&self, bucket: &str, key: &str, id: &str) -> Pending<ObjectInfo> {
        let (bucket, key, id) = (bucket.to_owned(), key.to_owned(), id.to_owned());
        self.spawn(|client| async move {
            copy(&client, &bucket, (&key, Some(&id)), &key, None).await?;
            head(&client, &bucket, &key).await
        })
    }

    /// Deletes version `id` of object `key` for good. Deleting the current version makes the one before current;
    /// deleting a current delete marker brings the object back.
    pub fn delete_version(&self, bucket: &str, key: &str, id: &str) -> Pending<()> {
        let (bucket, key, id) = (bucket.to_owned(), key.to_owned(), id.to_owned());
        self.spawn(|client| async move {
            client.delete_object().bucket(&bucket).key(&key).version_id(&id).send().await.map_err(map)?;
            Ok(())
        })
    }

    /// Whether bucket `bucket` keeps versions.
    pub fn versioning(&self, bucket: &str) -> Pending<Versioning> {
        let bucket = bucket.to_owned();
        self.spawn(|client| async move {
            let out = client.get_bucket_versioning().bucket(&bucket).send().await.map_err(map)?;
            Ok(match out.status() {
                Some(BucketVersioningStatus::Enabled) => Versioning::Enabled,
                Some(BucketVersioningStatus::Suspended) => Versioning::Suspended,
                _ => Versioning::Off,
            })
        })
    }

    /// Makes bucket `bucket` keep versions, or stop keeping them (`enabled` false): S3 can only suspend versioning,
    /// never turn it off, so the versions kept stay.
    pub fn set_versioning(&self, bucket: &str, enabled: bool) -> Pending<()> {
        let bucket = bucket.to_owned();
        self.spawn(|client| async move {
            let status = if enabled { BucketVersioningStatus::Enabled } else { BucketVersioningStatus::Suspended };
            let config = VersioningConfiguration::builder().status(status).build();
            client
                .put_bucket_versioning()
                .bucket(&bucket)
                .versioning_configuration(config)
                .send()
                .await
                .map_err(map)?;
            Ok(())
        })
    }
}

/// Whether `versions`, as [`Session::versions`] lists them, are worth showing: the bucket keeps (or kept) versions
/// of this object.
pub fn versioned(versions: &[Version]) -> bool {
    versions.len() > 1 || versions.iter().any(|v| v.id != "null")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn version(id: &str, latest: bool, secs: u64) -> Version {
        Version {
            id: id.to_owned(),
            latest,
            delete_marker: false,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(secs)),
            size: 1,
            etag: None,
        }
    }

    #[test]
    fn the_current_version_comes_first_then_the_newest() {
        let ids: Vec<_> = ordered(vec![version("a", false, 1), version("c", true, 2), version("b", false, 3)])
            .into_iter()
            .map(|v| v.id)
            .collect();
        assert_eq!(ids, ["c", "b", "a"]);
    }

    #[test]
    fn only_a_lone_null_version_is_unversioned() {
        assert!(!versioned(&[version("null", true, 1)]));
        assert!(!versioned(&[]));
        assert!(versioned(&[version("v1", true, 1)]));
        assert!(versioned(&[version("v2", true, 2), version("null", false, 1)]));
    }
}
