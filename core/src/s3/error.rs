//! What can go wrong talking to the gateway, by what it means to the developer rather than by the SDK's types.

use aws_sdk_s3::error::{ProvideErrorMetadata, SdkError};
use std::path::PathBuf;

/// Why an S3 operation failed.
#[derive(Debug, thiserror::Error)]
pub enum S3Error {
    /// The profile can't be used to connect (no credentials, unreadable files).
    #[error("profile can't connect: {0}")]
    Profile(String),
    /// The keys aren't the gateway's (unknown access key, wrong secret, clock far off).
    #[error("the gateway rejected the profile's keys: {0}")]
    BadCredentials(String),
    /// The profile's policies (or the bucket's) don't allow this. radosgw sends no reason, so the text is usually
    /// empty.
    #[error("not allowed by this profile's policies{}", if .0.is_empty() { String::new() } else { format!(" ({})", .0) })]
    AccessDenied(String),
    /// The bucket or object isn't there (any more).
    #[error("not found: {0}")]
    NotFound(String),
    /// Deleting a bucket that still has objects, versions or unfinished uploads.
    #[error("the bucket isn't empty")]
    BucketNotEmpty,
    /// Creating a bucket under a name that's taken.
    #[error("a bucket with that name already exists")]
    BucketExists,
    /// Renaming or moving onto a key (or folder) that's already there: rs3manager never overwrites silently.
    #[error("{0:?} already exists")]
    ObjectExists(String),
    /// A bucket name, key or metadata the gateway (or rs3manager, before sending it) refuses.
    #[error("invalid {0}")]
    Invalid(String),
    /// The gateway couldn't be reached, or stopped answering.
    #[error("can't reach the gateway: {0}")]
    Network(String),
    /// A local file couldn't be read or written.
    #[error("{path}: {source}")]
    Io {
        /// The file.
        path: PathBuf,
        /// What the system said.
        source: std::io::Error,
    },
    /// The user cancelled a transfer.
    #[error("cancelled")]
    Cancelled,
    /// Any other answer from the gateway, with its S3 error code.
    #[error("{code}: {message}")]
    Service {
        /// The S3 error code (`InternalError`, `SlowDown`, ...).
        code: String,
        /// Its message.
        message: String,
    },
    /// The task running the operation panicked: a bug in rs3manager.
    #[error("internal error: {0}")]
    Internal(String),
}

impl S3Error {
    /// An I/O error on `path`.
    pub(crate) fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io { path: path.into(), source }
    }
}

/// What an SDK error means: by its S3 error code when the gateway sent one, else by the HTTP status (HEAD answers
/// have no body), else as a network failure.
#[expect(clippy::needless_pass_by_value, reason = "taken by value to be passed as `map_err(map)`")]
pub(crate) fn map<E>(err: SdkError<E, aws_sdk_s3::config::http::HttpResponse>) -> S3Error
where
    E: ProvideErrorMetadata + std::error::Error + Send + Sync + 'static,
{
    let status = err.raw_response().map(|r| r.status().as_u16());
    if let Some(service) = err.as_service_error() {
        let message = service.message().unwrap_or_default().to_owned();
        if let Some(code) = service.code() {
            return by_code(code, message);
        }
        return by_status(status, message);
    }
    let message = aws_sdk_s3::error::DisplayErrorContext(&err).to_string();
    match err {
        SdkError::DispatchFailure(_) => S3Error::Network(root_cause(&err)),
        SdkError::TimeoutError(_) => S3Error::Network("the gateway didn't answer in time".to_owned()),
        // Building the request failed before sending it: no region or credentials resolved from the profile.
        SdkError::ConstructionFailure(_) => S3Error::Profile(message),
        _ => by_status(status, message),
    }
}

/// The error for an S3 error `code`.
fn by_code(code: &str, message: String) -> S3Error {
    let message = if message.is_empty() { code.to_owned() } else { message };
    match code {
        "NoSuchBucket" | "NoSuchKey" | "NoSuchUpload" | "NotFound" | "NoSuchVersion" => S3Error::NotFound(message),
        "AccessDenied" | "AllAccessDisabled" => {
            S3Error::AccessDenied(if message == code { String::new() } else { message })
        }
        "InvalidAccessKeyId" | "SignatureDoesNotMatch" | "RequestTimeTooSkewed" | "ExpiredToken" | "InvalidToken" => {
            S3Error::BadCredentials(message)
        }
        "BucketNotEmpty" => S3Error::BucketNotEmpty,
        "BucketAlreadyExists" | "BucketAlreadyOwnedByYou" => S3Error::BucketExists,
        "InvalidBucketName" | "KeyTooLongError" | "InvalidArgument" | "MetadataTooLarge" => S3Error::Invalid(message),
        _ => S3Error::Service { code: code.to_owned(), message },
    }
}

/// The error for an answer with no S3 error code, by its HTTP `status`.
fn by_status(status: Option<u16>, message: String) -> S3Error {
    match status {
        Some(404) => S3Error::NotFound(message),
        Some(403) => S3Error::AccessDenied(message),
        Some(401) => S3Error::BadCredentials(message),
        Some(code) => S3Error::Service { code: format!("HTTP {code}"), message },
        None => S3Error::Network(message),
    }
}

/// What went wrong at the bottom of `err`'s chain of causes, on one line, without the layers of wrappers the HTTP
/// stack puts around it (`dispatch failure: io error: client error (Connect): tcp connect error: Connection refused
/// (os error 111) (DispatchFailure(...))`): `Connection refused`.
fn root_cause(err: &(dyn std::error::Error + 'static)) -> String {
    let mut cause = err;
    while let Some(next) = cause.source() {
        cause = next;
    }
    let text = cause.to_string();
    // The system's code after its message adds nothing for a reader.
    let text = text.split(" (os error ").next().unwrap_or(&text).trim();
    if text.is_empty() { "no answer".to_owned() } else { text.to_owned() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt;

    /// An error wrapping another, as the HTTP stack's layers do.
    #[derive(Debug)]
    struct Layer(&'static str, Option<Box<dyn std::error::Error + 'static>>);

    impl fmt::Display for Layer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(self.0)
        }
    }

    impl std::error::Error for Layer {
        fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
            self.1.as_deref()
        }
    }

    #[test]
    fn an_unreachable_gateway_is_told_by_its_root_cause_alone() {
        let refused = Layer("Connection refused (os error 111)", None);
        let chain = Layer("dispatch failure", Some(Box::new(Layer("io error", Some(Box::new(refused))))));
        assert_eq!(root_cause(&chain), "Connection refused");
        assert_eq!(S3Error::Network(root_cause(&chain)).to_string(), "can't reach the gateway: Connection refused");
        assert_eq!(root_cause(&Layer("only", None)), "only");
        assert_eq!(root_cause(&Layer("", None)), "no answer");
    }

    #[test]
    fn failures_before_an_answer_are_the_network_or_the_profile() {
        use aws_sdk_s3::error::ConnectorError;
        use aws_sdk_s3::operation::get_object::GetObjectError;
        type Failure = SdkError<GetObjectError, aws_sdk_s3::config::http::HttpResponse>;
        let refused = ConnectorError::io(Box::new(std::io::Error::other("Connection refused (os error 111)")));
        assert!(matches!(map(Failure::dispatch_failure(refused)), S3Error::Network(m) if m == "Connection refused"));
        assert!(
            matches!(map(Failure::timeout_error(std::io::Error::other("slow"))), S3Error::Network(m) if m.contains("in time"))
        );
        assert!(matches!(map(Failure::construction_failure(std::io::Error::other("no region"))), S3Error::Profile(_)));
    }

    #[test]
    fn codes_map_to_what_they_mean() {
        assert!(matches!(by_code("NoSuchKey", String::new()), S3Error::NotFound(m) if m == "NoSuchKey"));
        assert!(matches!(by_code("SignatureDoesNotMatch", "x".into()), S3Error::BadCredentials(_)));
        assert!(matches!(by_code("BucketAlreadyOwnedByYou", String::new()), S3Error::BucketExists));
        assert!(matches!(by_code("SlowDown", "busy".into()), S3Error::Service { code, .. } if code == "SlowDown"));
        assert!(matches!(by_status(Some(404), String::new()), S3Error::NotFound(_)));
        assert!(matches!(by_status(None, String::new()), S3Error::Network(_)));
        assert!(matches!(by_code("BucketNotEmpty", String::new()), S3Error::BucketNotEmpty));
        for code in ["InvalidBucketName", "KeyTooLongError", "InvalidArgument", "MetadataTooLarge"] {
            assert!(matches!(by_code(code, "why".into()), S3Error::Invalid(m) if m == "why"), "{code}");
        }
        // radosgw denies with no message: the text says why without repeating the code.
        assert_eq!(by_code("AccessDenied", String::new()).to_string(), "not allowed by this profile's policies");
    }
}
