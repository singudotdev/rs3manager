//! The S3 operations a developer runs on their radosgw account: buckets, folders and objects, their metadata, and
//! uploads and downloads with progress.
//!
//! The SDK needs tokio, and the app runs on GPUI's executor. [`Engine`] owns a tokio runtime on its own threads;
//! every operation of a [`Session`] is spawned there and handed back as a [`Pending`], a plain future any executor
//! can await, so tokio never runs inside GPUI's async code (see `docs/decisions/0001-*`).
//!
//! - [`error`]: what a failure means.
//! - [`names`]: bucket names, keys and metadata checked before they're sent.
//! - `ops`: buckets, listings, metadata, rename, delete.
//! - `transfer`: uploads, downloads and their progress.

mod error;
pub mod names;
mod ops;
mod transfer;

pub use error::S3Error;
pub use transfer::{Progress, local_path_for, walk};

use crate::profiles::{DEFAULT_REGION, Files};
use aws_runtime::env_config::file::{EnvConfigFileKind, EnvConfigFiles};
use aws_sdk_s3::config::{Region, RequestChecksumCalculation, ResponseChecksumValidation};
use std::collections::BTreeMap;
use std::pin::Pin;
use std::task::{Context, Poll};
use std::time::SystemTime;

/// The tokio runtime the SDK runs on, away from the UI's threads. Dropping it stops every operation still running;
/// it must be dropped outside any async context (the app keeps it for its whole life).
pub struct Engine {
    /// The runtime: two worker threads, as the work is network-bound.
    runtime: tokio::runtime::Runtime,
}

impl Engine {
    /// Starts the runtime's threads.
    pub fn new() -> Result<Self, S3Error> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("r3manager-s3")
            .enable_all()
            .build()
            .map_err(|e| S3Error::Internal(format!("couldn't start the S3 threads: {e}")))?;
        Ok(Self { runtime })
    }

    /// A session as profile `profile` of `files`, loaded by the SDK itself so whatever the AWS CLI accepts there works
    /// (static keys, SSO, `credential_process`, a `services` endpoint). Path-style addressing is forced, since
    /// radosgw is usually served without wildcard DNS, and checksums are only sent where S3 requires them, which
    /// radosgw releases before Squid don't all accept.
    pub fn connect(&self, files: &Files, profile: &str) -> Pending<Session> {
        let profile_files = EnvConfigFiles::builder()
            .with_file(EnvConfigFileKind::Config, &files.config)
            .with_file(EnvConfigFileKind::Credentials, &files.credentials)
            .build();
        let name = profile.to_owned();
        let handle = self.runtime.handle().clone();
        Pending(self.runtime.spawn(async move {
            let shared = aws_config::defaults(aws_config::BehaviorVersion::latest())
                .profile_files(profile_files)
                .profile_name(&name)
                .load()
                .await;
            if shared.credentials_provider().is_none() {
                return Err(S3Error::Profile(format!("profile {name:?} has no credentials")));
            }
            let mut config = aws_sdk_s3::config::Builder::from(&shared)
                .force_path_style(true)
                .request_checksum_calculation(RequestChecksumCalculation::WhenRequired)
                .response_checksum_validation(ResponseChecksumValidation::WhenRequired);
            if shared.region().is_none() {
                config = config.region(Region::new(DEFAULT_REGION));
            }
            let endpoint = shared.endpoint_url().map(str::to_owned);
            tracing::info!(target: "r3manager::s3", profile = %name, endpoint = ?endpoint, "connected");
            Ok(Session { client: aws_sdk_s3::Client::from_conf(config.build()), handle, profile: name, endpoint })
        }))
    }
}

/// The profile a developer works as: every bucket and object operation, run on the [`Engine`]'s runtime. Cheap to
/// clone; clones share the connection pool.
#[derive(Clone)]
pub struct Session {
    /// The SDK's client.
    client: aws_sdk_s3::Client,
    /// Where operations are spawned.
    handle: tokio::runtime::Handle,
    /// The profile's name.
    profile: String,
    /// The endpoint the SDK resolved from the profile, if one is set.
    endpoint: Option<String>,
}

impl Session {
    /// The profile's name.
    pub fn profile(&self) -> &str {
        &self.profile
    }

    /// The gateway's URL, when the profile sets one (else the SDK's AWS default).
    pub fn endpoint(&self) -> Option<&str> {
        self.endpoint.as_deref()
    }

    /// Runs `work` with the client on the engine's runtime.
    fn spawn<T, F>(&self, work: impl FnOnce(aws_sdk_s3::Client) -> F) -> Pending<T>
    where
        T: Send + 'static,
        F: Future<Output = Result<T, S3Error>> + Send + 'static,
    {
        Pending(self.handle.spawn(work(self.client.clone())))
    }
}

/// An operation running on the [`Engine`]: await it from any executor. Dropping it lets the operation finish
/// unobserved; cancel transfers through their [`Progress`].
#[must_use = "an operation's result says whether it worked"]
pub struct Pending<T>(tokio::task::JoinHandle<Result<T, S3Error>>);

impl<T> Future for Pending<T> {
    type Output = Result<T, S3Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        Pin::new(&mut self.0).poll(cx).map(|joined| {
            joined.unwrap_or_else(|e| {
                Err(if e.is_cancelled() { S3Error::Cancelled } else { S3Error::Internal(e.to_string()) })
            })
        })
    }
}

/// A bucket the account owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bucket {
    /// Its name.
    pub name: String,
    /// When it was created.
    pub created: Option<SystemTime>,
}

/// An object in a listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectEntry {
    /// Its full key.
    pub key: String,
    /// Its size in bytes.
    pub size: u64,
    /// When it was last written.
    pub modified: Option<SystemTime>,
}

/// One page of a folder: its subfolders and objects, sorted by key as S3 returns them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Page {
    /// The full prefixes of the folders in it, each ending in `/`.
    pub folders: Vec<String>,
    /// Its objects. A folder's own marker object (its prefix, zero bytes) isn't among them.
    pub objects: Vec<ObjectEntry>,
    /// The token for the next page, when there's more.
    pub next: Option<String>,
}

/// The headers of an object a developer sets: those apps and browsers read when fetching it, and user metadata.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Headers {
    /// `Content-Type`.
    pub content_type: Option<String>,
    /// `Cache-Control`.
    pub cache_control: Option<String>,
    /// `Content-Disposition`.
    pub content_disposition: Option<String>,
    /// `Content-Encoding`.
    pub content_encoding: Option<String>,
    /// `Content-Language`.
    pub content_language: Option<String>,
    /// User metadata, `x-amz-meta-NAME`, names lowercase.
    pub user: BTreeMap<String, String>,
}

impl Headers {
    /// Checks every value before it's sent: header values on one line, user metadata as [`names::check_metadata`].
    pub fn check(&self) -> Result<(), S3Error> {
        let fixed = [
            ("Content-Type", &self.content_type),
            ("Cache-Control", &self.cache_control),
            ("Content-Disposition", &self.content_disposition),
            ("Content-Encoding", &self.content_encoding),
            ("Content-Language", &self.content_language),
        ];
        for (name, value) in fixed {
            if value.as_deref().is_some_and(|v| v.chars().any(char::is_control)) {
                return Err(S3Error::Invalid(format!("{name}: it must be on one line")));
            }
        }
        self.user.iter().try_for_each(|(name, value)| names::check_metadata(name, value))
    }
}

/// An object's details.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObjectInfo {
    /// Its key.
    pub key: String,
    /// Its size in bytes.
    pub size: u64,
    /// When it was last written.
    pub modified: Option<SystemTime>,
    /// Its `ETag`, without quotes.
    pub etag: Option<String>,
    /// Its version, when the bucket keeps versions.
    pub version_id: Option<String>,
    /// Its storage class, when not the default.
    pub storage_class: Option<String>,
    /// Its headers and user metadata.
    pub headers: Headers,
}

/// The first bytes of an object, to show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    /// Up to the limit asked for.
    pub bytes: Vec<u8>,
    /// Whether the object is longer than that.
    pub truncated: bool,
}

/// What a delete of several objects did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeleteReport {
    /// How many objects are gone.
    pub deleted: u64,
    /// The keys that couldn't be deleted, with why.
    pub failed: Vec<(String, String)>,
}

/// A time from the SDK as the system's, `None` when out of range.
fn time(t: Option<&aws_sdk_s3::primitives::DateTime>) -> Option<SystemTime> {
    t.and_then(|t| SystemTime::try_from(*t).ok())
}

/// A size from the SDK, which gives them signed.
fn size(s: Option<i64>) -> u64 {
    s.and_then(|s| u64::try_from(s).ok()).unwrap_or(0)
}
