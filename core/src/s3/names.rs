//! Names and values checked before they're sent, so the developer sees why at once instead of a gateway error code:
//! bucket names, keys, user metadata. Also the encodings S3 needs for them.

use super::S3Error;
use std::fmt::Write as _;

/// Checks a new bucket's name against S3's [naming rules](https://docs.aws.amazon.com/AmazonS3/latest/userguide/bucketnamingrules.html),
/// which radosgw enforces too: 3 to 63 lowercase letters, digits, `-` and `.`, starting and ending with a letter or
/// digit, no `..`, and not shaped like an IP address.
pub fn check_bucket_name(name: &str) -> Result<(), S3Error> {
    let invalid = |why: &str| Err(S3Error::Invalid(format!("bucket name {name:?}: {why}")));
    if !(3..=63).contains(&name.len()) {
        return invalid("it must be 3 to 63 characters long");
    }
    if !name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.') {
        return invalid("use lowercase letters, digits, '-' and '.' only");
    }
    let edge = |b: u8| b.is_ascii_lowercase() || b.is_ascii_digit();
    if !edge(name.as_bytes()[0]) || !edge(name.as_bytes()[name.len() - 1]) {
        return invalid("it must start and end with a letter or digit");
    }
    if name.contains("..") || name.contains(".-") || name.contains("-.") {
        return invalid("no '..', '.-' or '-.'");
    }
    if name.parse::<std::net::Ipv4Addr>().is_ok() {
        return invalid("it can't look like an IP address");
    }
    Ok(())
}

/// Checks an object key: not empty, at most 1024 bytes of UTF-8 (S3's limit), and no control characters, which
/// radosgw accepts but no file browser or HTTP header shows sanely.
pub fn check_key(key: &str) -> Result<(), S3Error> {
    if key.is_empty() {
        return Err(S3Error::Invalid("object name: it can't be empty".to_owned()));
    }
    if key.len() > 1024 {
        return Err(S3Error::Invalid("object name: at most 1024 bytes".to_owned()));
    }
    if key.chars().any(char::is_control) {
        return Err(S3Error::Invalid(format!("object name {key:?}: no control characters")));
    }
    Ok(())
}

/// Checks a user metadata entry (`x-amz-meta-NAME: value`): the name an HTTP header token, the value printable
/// US-ASCII, as S3 sends [user-defined metadata](https://docs.aws.amazon.com/AmazonS3/latest/userguide/UsingMetadata.html#UserMetadata)
/// over REST.
pub fn check_metadata(name: &str, value: &str) -> Result<(), S3Error> {
    let token = |c: char| c.is_ascii_alphanumeric() || "!#$%&'*+-.^_`|~".contains(c);
    if name.is_empty() || !name.chars().all(token) {
        return Err(S3Error::Invalid(format!("metadata name {name:?}: letters, digits and '-' only")));
    }
    if !value.chars().all(|c| c == ' ' || c.is_ascii_graphic()) {
        return Err(S3Error::Invalid(format!("metadata {name:?}: the value must be plain ASCII text")));
    }
    Ok(())
}

/// `path` percent-encoded for a URL path, `/` and RFC 3986's unreserved characters kept: an object's URL on the
/// gateway, and the copy source below.
pub fn url_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for byte in path.bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            out.push(char::from(byte));
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// The `x-amz-copy-source` value for `key` in `bucket`: URL-encoded, with `/` kept, as
/// [CopyObject](https://docs.aws.amazon.com/AmazonS3/latest/API/API_CopyObject.html) requires.
pub(crate) fn copy_source(bucket: &str, key: &str) -> String {
    url_path(&format!("{bucket}/{key}"))
}

/// The Content-Type for a file named `name`, by its extension: what a browser or app fetching the object needs to
/// show it. Unknown ones are `application/octet-stream`.
pub fn content_type_for(name: &str) -> &'static str {
    let ext = name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default();
    match ext.as_str() {
        "html" | "htm" => "text/html; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "json" | "map" => "application/json",
        "txt" | "log" | "md" | "csv" | "tsv" | "ini" | "conf" | "toml" | "yaml" | "yml" => match ext.as_str() {
            "csv" => "text/csv; charset=utf-8",
            "md" => "text/markdown; charset=utf-8",
            "yaml" | "yml" => "application/yaml",
            _ => "text/plain; charset=utf-8",
        },
        "xml" => "application/xml",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "ico" => "image/x-icon",
        "bmp" => "image/bmp",
        "tif" | "tiff" => "image/tiff",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        "tar" => "application/x-tar",
        "zst" => "application/zstd",
        "wasm" => "application/wasm",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mp3" => "audio/mpeg",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bucket_names_follow_s3s_rules() {
        for good in ["abc", "my-bucket.v2", "a1b", &"a".repeat(63)] {
            assert!(check_bucket_name(good).is_ok(), "{good}");
        }
        for bad in ["ab", "My-Bucket", "-abc", "abc-", "a..b", "a.-b", "192.168.1.1", "a_b", &"a".repeat(64)] {
            assert!(check_bucket_name(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn keys_and_metadata_are_checked() {
        assert!(check_key("dir/file name ñ.txt").is_ok());
        assert!(check_key("").is_err());
        assert!(check_key("a\nb").is_err());
        assert!(check_key(&"k".repeat(1025)).is_err());
        assert!(check_metadata("owner-team", "Platform 2").is_ok());
        assert!(check_metadata("bad name", "x").is_err());
        assert!(check_metadata("x", "café").is_err());
    }

    #[test]
    fn copy_sources_are_url_encoded_keeping_slashes() {
        assert_eq!(copy_source("b", "dir/a b+c%ñ.txt"), "b/dir/a%20b%2Bc%25%C3%B1.txt");
    }

    #[test]
    fn content_types_by_extension() {
        assert_eq!(content_type_for("a/b/Photo.JPG"), "image/jpeg");
        assert_eq!(content_type_for("data.json"), "application/json");
        assert_eq!(content_type_for("README"), "application/octet-stream");
    }
}
