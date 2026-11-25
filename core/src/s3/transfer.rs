//! Uploads and downloads between local files and objects, with progress the UI reads while they run and a flag to
//! cancel them. Large files go up in parts, several at a time; downloads land in a temporary file renamed into place
//! once complete, so a cancelled or failed one never leaves a truncated file under the real name. One that was cancelled
//! or lost its connection keeps what it wrote, and the next download of the same object continues from there.

use super::error::map;
use super::names::check_key;
use super::ops::with_headers;
use super::{Headers, ObjectEntry, Pending, S3Error, Session, size, time};
use aws_sdk_s3::Client;
use aws_sdk_s3::primitives::{ByteStream, Length};
use aws_sdk_s3::types::{CompletedMultipartUpload, CompletedPart};
use std::collections::{BTreeSet, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use tokio::io::AsyncWriteExt as _;

/// Files up to this size go up in one `PutObject`; larger ones in parts.
const MULTIPART_THRESHOLD: u64 = 16 * 1024 * 1024;

/// The smallest part rs3manager sends: S3's minimum is 5 MiB, and fewer, larger requests go faster.
const MIN_PART: u64 = 8 * 1024 * 1024;

/// S3's limit of parts in one upload.
const MAX_PARTS: u64 = 10_000;

/// Parts of one upload in flight at once.
const PARTS_IN_FLIGHT: usize = 4;

/// How far a transfer is, shared between the task doing it and the UI showing it.
#[derive(Debug, Default)]
pub struct Progress {
    /// Bytes moved so far.
    done: AtomicU64,
    /// Bytes to move, once known.
    total: AtomicU64,
    /// Set by [`Progress::cancel`]; the transfer stops at its next chunk or part.
    cancelled: AtomicBool,
}

impl Progress {
    /// A transfer that hasn't started.
    pub fn new() -> Arc<Self> {
        Arc::default()
    }

    /// Bytes moved and bytes to move.
    pub fn get(&self) -> (u64, u64) {
        (self.done.load(Ordering::Relaxed), self.total.load(Ordering::Relaxed))
    }

    /// Asks the transfer to stop; it ends with [`S3Error::Cancelled`].
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }

    /// Fails with [`S3Error::Cancelled`] once cancelled.
    fn check(&self) -> Result<(), S3Error> {
        if self.cancelled.load(Ordering::Relaxed) { Err(S3Error::Cancelled) } else { Ok(()) }
    }

    /// Records `n` more bytes moved.
    fn add(&self, n: u64) {
        self.done.fetch_add(n, Ordering::Relaxed);
    }
}

/// The part size for an upload of `len` bytes: [`MIN_PART`], or more so it fits in [`MAX_PARTS`], in whole MiB.
pub(crate) fn part_size(len: u64) -> u64 {
    const MIB: u64 = 1024 * 1024;
    len.div_ceil(MAX_PARTS).div_ceil(MIB).saturating_mul(MIB).max(MIN_PART)
}

impl Session {
    /// Uploads file `path` as object `key` of `bucket` with `headers`, reporting to `progress`.
    pub fn upload(
        &self,
        bucket: &str,
        key: &str,
        path: &Path,
        headers: Headers,
        progress: Arc<Progress>,
    ) -> Pending<()> {
        let (bucket, key, path) = (bucket.to_owned(), key.to_owned(), path.to_owned());
        self.spawn(|client| async move {
            check_key(&key)?;
            headers.check()?;
            let len = tokio::fs::metadata(&path).await.map_err(|e| S3Error::io(&path, e))?.len();
            progress.total.store(len, Ordering::Relaxed);
            progress.check()?;
            if len <= MULTIPART_THRESHOLD {
                let body = ByteStream::from_path(&path)
                    .await
                    .map_err(|e| S3Error::Io { path: path.clone(), source: std::io::Error::other(e) })?;
                with_headers!(client.put_object().bucket(&bucket).key(&key).body(body), &headers)
                    .send()
                    .await
                    .map_err(map)?;
                progress.add(len);
                return Ok(());
            }
            let created = with_headers!(client.create_multipart_upload().bucket(&bucket).key(&key), &headers)
                .send()
                .await
                .map_err(map)?;
            let upload = Upload {
                client: client.clone(),
                bucket: bucket.clone(),
                key: key.clone(),
                id: created.upload_id().unwrap_or_default().to_owned(),
                path,
            };
            let result = upload.parts(len, &progress).await;
            if result.is_err() {
                let _ = client.abort_multipart_upload().bucket(&bucket).key(&key).upload_id(&upload.id).send().await;
            }
            result
        })
    }

    /// Downloads object `key` of `bucket` to file `dest`, creating its folder, reporting to `progress`. An existing
    /// `dest` is replaced only once the download is complete.
    pub fn download(&self, bucket: &str, key: &str, dest: &Path, progress: Arc<Progress>) -> Pending<()> {
        let (bucket, key, dest) = (bucket.to_owned(), key.to_owned(), dest.to_owned());
        self.spawn(|client| async move {
            let partial = partial_path(&dest);
            let result = download(&client, &bucket, &key, &dest, &partial, &progress).await;
            // What was written stays for the next try after a cancel or a lost connection; any other failure
            // (denied, gone, a full disk) would only fail again.
            if result.is_err() && !matches!(result, Err(S3Error::Cancelled | S3Error::Network(_))) {
                forget(&partial).await;
            }
            result
        })
    }

    /// Which of `keys` already exist in `bucket`, so an upload can ask before replacing them (S3 replaces silently).
    /// One listing per folder the keys go into, not a request per key; a folder found empty skips every folder below
    /// it, so a new folder tree costs one listing. Keys written by someone else after this answered aren't seen.
    pub fn existing(&self, bucket: &str, keys: Vec<String>) -> Pending<Vec<String>> {
        let bucket = bucket.to_owned();
        self.spawn(|client| async move {
            let folders: BTreeSet<&str> = keys.iter().map(|k| k.rfind('/').map_or("", |i| &k[..=i])).collect();
            let mut found = HashSet::new();
            let mut empty: Vec<&str> = Vec::new();
            // Sorted, so a folder comes before the folders below it.
            for folder in folders {
                if empty.iter().any(|e| folder.starts_with(e)) {
                    continue;
                }
                let mut token = None;
                let mut anything = false;
                loop {
                    let page = client
                        .list_objects_v2()
                        .bucket(&bucket)
                        .prefix(folder)
                        .delimiter("/")
                        .set_continuation_token(token)
                        .send()
                        .await
                        .map_err(map)?;
                    anything |= !page.contents().is_empty() || !page.common_prefixes().is_empty();
                    found.extend(page.contents().iter().filter_map(|o| o.key().map(str::to_owned)));
                    token = page.next_continuation_token().map(str::to_owned);
                    if token.is_none() {
                        break;
                    }
                }
                if !anything && !folder.is_empty() {
                    empty.push(folder);
                }
            }
            Ok(keys.iter().filter(|k| found.contains(k.as_str())).cloned().collect())
        })
    }

    /// Every object under `prefix` of `bucket`, at any depth: what downloading or deleting a folder works on.
    pub fn list_recursive(&self, bucket: &str, prefix: &str) -> Pending<Vec<ObjectEntry>> {
        let (bucket, prefix) = (bucket.to_owned(), prefix.to_owned());
        self.spawn(|client| async move {
            let mut objects = Vec::new();
            let mut token = None;
            loop {
                let page = client
                    .list_objects_v2()
                    .bucket(&bucket)
                    .prefix(&prefix)
                    .set_continuation_token(token)
                    .send()
                    .await
                    .map_err(map)?;
                objects.extend(page.contents().iter().filter_map(|o| {
                    Some(ObjectEntry {
                        key: o.key()?.to_owned(),
                        size: size(o.size()),
                        modified: time(o.last_modified()),
                    })
                }));
                token = page.next_continuation_token().map(str::to_owned);
                if token.is_none() {
                    return Ok(objects);
                }
            }
        })
    }
}

/// A multipart upload under way.
struct Upload {
    /// The client sending it.
    client: Client,
    /// The bucket.
    bucket: String,
    /// The object's key.
    key: String,
    /// The upload's id.
    id: String,
    /// The local file.
    path: PathBuf,
}

impl Upload {
    /// Sends the file's `len` bytes in parts, [`PARTS_IN_FLIGHT`] at a time, then completes the upload.
    async fn parts(&self, len: u64, progress: &Arc<Progress>) -> Result<(), S3Error> {
        let part = part_size(len);
        let count = len.div_ceil(part);
        let mut running = tokio::task::JoinSet::new();
        let mut done = Vec::with_capacity(usize::try_from(count).unwrap_or(0));
        let mut next = 0;
        while next < count || !running.is_empty() {
            while next < count && running.len() < PARTS_IN_FLIGHT {
                progress.check()?;
                let offset = next * part;
                let request = self.part(next + 1, offset, part.min(len - offset), Arc::clone(progress));
                running.spawn(request);
                next += 1;
            }
            if let Some(joined) = running.join_next().await {
                done.push(joined.map_err(|e| S3Error::Internal(e.to_string()))??);
            }
        }
        done.sort_by_key(|p: &CompletedPart| p.part_number());
        self.client
            .complete_multipart_upload()
            .bucket(&self.bucket)
            .key(&self.key)
            .upload_id(&self.id)
            .multipart_upload(CompletedMultipartUpload::builder().set_parts(Some(done)).build())
            .send()
            .await
            .map_err(map)?;
        Ok(())
    }

    /// The request sending part `number`: `len` bytes of the file from `offset`.
    fn part(
        &self,
        number: u64,
        offset: u64,
        len: u64,
        progress: Arc<Progress>,
    ) -> impl Future<Output = Result<CompletedPart, S3Error>> + Send + 'static {
        let (client, bucket, key, id, path) =
            (self.client.clone(), self.bucket.clone(), self.key.clone(), self.id.clone(), self.path.clone());
        async move {
            let number = i32::try_from(number).map_err(|_| S3Error::Invalid("too many parts".to_owned()))?;
            let body = ByteStream::read_from()
                .path(&path)
                .offset(offset)
                .length(Length::Exact(len))
                .build()
                .await
                .map_err(|e| S3Error::Io { path: path.clone(), source: std::io::Error::other(e) })?;
            let out = client
                .upload_part()
                .bucket(&bucket)
                .key(&key)
                .upload_id(&id)
                .part_number(number)
                .body(body)
                .send()
                .await
                .map_err(map)?;
            progress.add(len);
            Ok(CompletedPart::builder().part_number(number).set_e_tag(out.e_tag().map(str::to_owned)).build())
        }
    }
}

/// Where a download to `dest` is written until complete: beside it, hidden, so the rename into place is atomic.
fn partial_path(dest: &Path) -> PathBuf {
    let name = dest.file_name().map_or_else(|| "download".into(), |n| n.to_string_lossy());
    dest.with_file_name(format!(".{name}.rs3manager-part"))
}

/// The file holding the `ETag` of the object `partial` is a piece of, so a later download only continues one that's
/// still the same object.
fn etag_path(partial: &Path) -> PathBuf {
    let mut name = partial.as_os_str().to_owned();
    name.push(".etag");
    PathBuf::from(name)
}

/// Removes `partial` and its `ETag` file.
async fn forget(partial: &Path) {
    let _ = tokio::fs::remove_file(partial).await;
    let _ = tokio::fs::remove_file(etag_path(partial)).await;
}

/// How much of the object an earlier try left in `partial`, with the `ETag` it had then, if both are still there.
async fn resumable(partial: &Path) -> Option<(u64, String)> {
    let len = tokio::fs::metadata(partial).await.ok()?.len();
    let etag = tokio::fs::read_to_string(etag_path(partial)).await.ok()?;
    (len > 0 && !etag.is_empty()).then_some((len, etag))
}

/// Streams object `key` into `partial`, then renames it to `dest`. Continues what an earlier try left in `partial`
/// when the object is unchanged (`If-Match` its `ETag`) and the gateway honors the range; otherwise starts over.
async fn download(
    client: &Client,
    bucket: &str,
    key: &str,
    dest: &Path,
    partial: &Path,
    progress: &Progress,
) -> Result<(), S3Error> {
    progress.check()?;
    let mut have = 0;
    let mut out = None;
    if let Some((len, etag)) = resumable(partial).await {
        let request = client.get_object().bucket(bucket).key(key).range(format!("bytes={len}-")).if_match(etag);
        if let Ok(resumed) = request.send().await {
            have = len;
            out = Some(resumed);
        }
    }
    let mut out = match out {
        Some(out) => out,
        None => client.get_object().bucket(bucket).key(key).send().await.map_err(map)?,
    };
    progress.total.store(have + size(out.content_length()), Ordering::Relaxed);
    progress.done.store(have, Ordering::Relaxed);
    if let Some(dir) = dest.parent().filter(|d| !d.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(dir).await.map_err(|e| S3Error::io(dir, e))?;
    }
    let mut file = if have > 0 {
        tokio::fs::OpenOptions::new().append(true).open(partial).await.map_err(|e| S3Error::io(partial, e))?
    } else {
        let etag = out.e_tag().unwrap_or_default();
        tokio::fs::write(etag_path(partial), etag).await.map_err(|e| S3Error::io(partial, e))?;
        tokio::fs::File::create(partial).await.map_err(|e| S3Error::io(partial, e))?
    };
    while let Some(chunk) = out.body.next().await {
        progress.check()?;
        let chunk = chunk.map_err(|e| S3Error::Network(format!("reading {key}: {e}")))?;
        file.write_all(&chunk).await.map_err(|e| S3Error::io(partial, e))?;
        progress.add(chunk.len() as u64);
    }
    file.flush().await.map_err(|e| S3Error::io(partial, e))?;
    file.sync_all().await.map_err(|e| S3Error::io(partial, e))?;
    drop(file);
    tokio::fs::rename(partial, dest).await.map_err(|e| S3Error::io(dest, e))?;
    let _ = tokio::fs::remove_file(etag_path(partial)).await;
    Ok(())
}

/// Every file under `dir`, with its path relative to `dir` as `/`-separated key segments, sorted: what uploading a
/// folder sends. Symbolic links are skipped, so a link can't loop the walk or pull in files from outside `dir`.
pub fn walk(dir: &Path) -> std::io::Result<Vec<(PathBuf, String)>> {
    let mut files = Vec::new();
    let mut pending = vec![dir.to_owned()];
    while let Some(folder) = pending.pop() {
        for entry in std::fs::read_dir(&folder)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                let path = entry.path();
                let relative = path.strip_prefix(dir).map_err(std::io::Error::other)?;
                let key = relative.components().map(|c| c.as_os_str().to_string_lossy()).collect::<Vec<_>>().join("/");
                files.push((path, key));
            }
        }
    }
    files.sort_by(|a, b| a.1.cmp(&b.1));
    Ok(files)
}

/// Where object `key`, relative to the folder being downloaded, goes under `base`; `None` for keys that would land
/// outside it (`..`, absolute or drive paths, backslashes) or that aren't files (empty segments, a folder marker). A
/// bucket's keys are anyone's who can write to it: they must never choose where the download writes.
pub fn local_path_for(base: &Path, key: &str) -> Option<PathBuf> {
    let mut path = base.to_owned();
    for segment in key.split('/') {
        if segment.is_empty() || segment.contains(['\\', ':', '\0']) {
            return None;
        }
        let mut components = Path::new(segment).components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(name)), None) => path.push(name),
            _ => return None,
        }
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_stay_within_s3s_limits() {
        const MIB: u64 = 1024 * 1024;
        assert_eq!(part_size(20 * MIB), MIN_PART);
        let huge = 200 * 1024 * MIB;
        let part = part_size(huge);
        assert!(huge.div_ceil(part) <= MAX_PARTS);
        assert_eq!(part % MIB, 0);
    }

    #[test]
    fn keys_can_never_choose_where_a_download_writes() {
        let base = Path::new("/home/dev/Downloads/x");
        assert_eq!(local_path_for(base, "a/b.txt"), Some(base.join("a").join("b.txt")));
        for bad in ["../evil", "a/../../evil", "/etc/passwd", "a//b", "a/", "", ".", "a\\..\\b", "C:x", "a/./b"] {
            assert_eq!(local_path_for(base, bad), None, "{bad}");
        }
    }

    #[test]
    fn walking_a_folder_gives_relative_keys_and_skips_links() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("sub/deep")).unwrap();
        std::fs::write(dir.path().join("top.txt"), "a").unwrap();
        std::fs::write(dir.path().join("sub/deep/x.bin"), "b").unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/etc", dir.path().join("link")).unwrap();
        let keys: Vec<_> = walk(dir.path()).unwrap().into_iter().map(|(_, k)| k).collect();
        assert_eq!(keys, ["sub/deep/x.bin", "top.txt"]);
    }

    #[test]
    fn progress_counts_and_cancels() {
        let p = Progress::new();
        p.total.store(10, Ordering::Relaxed);
        p.add(4);
        assert_eq!(p.get(), (4, 10));
        assert!(p.check().is_ok());
        p.cancel();
        assert!(matches!(p.check(), Err(S3Error::Cancelled)));
    }
}
