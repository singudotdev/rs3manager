//! A bucket's settings a developer edits instead of using the AWS CLI: its tags, lifecycle rules and CORS rules. Each
//! is read whole and written whole, as S3 does; writing none removes it. Bucket policies stay out (decision #29).
//!
//! Lifecycle rules hold more than the window edits (transitions, tag and size filters, dates). A rule with any of that
//! is read as [`LifecycleRule::kept`] and written back as it came, so saving from the window never drops what the CLI
//! set.

use super::error::map;
use super::{Pending, S3Error, Session};
use aws_sdk_s3::types::{
    AbortIncompleteMultipartUpload, BucketLifecycleConfiguration, CorsConfiguration, ExpirationStatus,
    LifecycleExpiration, LifecycleRuleFilter, NoncurrentVersionExpiration, Tag, Tagging,
};

/// Most tags a bucket has.
pub const MAX_TAGS: usize = 50;
/// Most characters in a tag's key.
const TAG_KEY_MAX: usize = 128;
/// Most characters in a tag's value.
const TAG_VALUE_MAX: usize = 256;
/// Most characters in a lifecycle rule's ID.
const RULE_ID_MAX: usize = 255;
/// Most lifecycle rules a bucket has.
const MAX_LIFECYCLE_RULES: usize = 1000;
/// Most CORS rules a bucket has.
const MAX_CORS_RULES: usize = 100;
/// The methods a CORS rule may allow.
pub const CORS_METHODS: [&str; 5] = ["GET", "PUT", "POST", "DELETE", "HEAD"];

/// A bucket's tag: a key and its value.
pub type BucketTag = (String, String);

/// A lifecycle rule, as far as the window edits it.
#[derive(Debug, Clone, PartialEq)]
pub struct LifecycleRule {
    /// Its ID, unique in the bucket.
    pub id: String,
    /// The keys it applies to: those starting with this, every key when empty.
    pub prefix: String,
    /// Whether it runs.
    pub enabled: bool,
    /// Days after which objects expire (the current version, in a versioned bucket).
    pub expire_days: Option<u32>,
    /// Days after which a version no longer current is deleted.
    pub noncurrent_days: Option<u32>,
    /// Days after which an unfinished multipart upload is aborted.
    pub abort_days: Option<u32>,
    /// The rule as read, when it holds what the fields above can't: then it's shown, not edited, and saved back as
    /// it came.
    pub kept: Option<KeptRule>,
}

/// A lifecycle rule kept as the gateway sent it.
#[derive(Debug, Clone, PartialEq)]
pub struct KeptRule {
    /// The rule.
    rule: aws_sdk_s3::types::LifecycleRule,
    /// What it holds that the window doesn't edit.
    pub extras: Vec<&'static str>,
}

impl LifecycleRule {
    /// A new, enabled rule with nothing set.
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            prefix: String::new(),
            enabled: true,
            expire_days: None,
            noncurrent_days: None,
            abort_days: None,
            kept: None,
        }
    }
}

/// A CORS rule: which web origins may make which requests to the bucket's objects from a browser.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CorsRule {
    /// Its ID, optional.
    pub id: Option<String>,
    /// The origins allowed, e.g. `https://app.example.com` or `*`.
    pub origins: Vec<String>,
    /// The methods allowed, among [`CORS_METHODS`].
    pub methods: Vec<String>,
    /// The request headers allowed in a preflight (`Access-Control-Request-Headers`).
    pub allowed_headers: Vec<String>,
    /// The response headers the browser may read.
    pub expose_headers: Vec<String>,
    /// How long a browser may cache the preflight answer, in seconds.
    pub max_age: Option<u32>,
}

/// Checks `tags` before they're sent: keys present and unique, lengths and count within S3's limits.
pub fn check_tags(tags: &[BucketTag]) -> Result<(), S3Error> {
    if tags.len() > MAX_TAGS {
        return Err(S3Error::Invalid(format!("tags: a bucket has at most {MAX_TAGS}")));
    }
    for (i, (key, value)) in tags.iter().enumerate() {
        if key.trim().is_empty() {
            return Err(S3Error::Invalid("tag: the key is empty".to_owned()));
        }
        if key.chars().count() > TAG_KEY_MAX {
            return Err(S3Error::Invalid(format!("tag {key:?}: keys are at most {TAG_KEY_MAX} characters")));
        }
        if value.chars().count() > TAG_VALUE_MAX {
            return Err(S3Error::Invalid(format!("tag {key:?}: values are at most {TAG_VALUE_MAX} characters")));
        }
        if key.starts_with("aws:") {
            return Err(S3Error::Invalid(format!("tag {key:?}: keys starting with aws: are reserved")));
        }
        if tags[..i].iter().any(|(k, _)| k == key) {
            return Err(S3Error::Invalid(format!("tag {key:?}: the key is there twice")));
        }
    }
    Ok(())
}

/// Checks `rules` before they're sent: IDs present and unique, each edited rule doing something.
pub fn check_lifecycle(rules: &[LifecycleRule]) -> Result<(), S3Error> {
    if rules.len() > MAX_LIFECYCLE_RULES {
        return Err(S3Error::Invalid(format!("lifecycle: a bucket has at most {MAX_LIFECYCLE_RULES} rules")));
    }
    for (i, rule) in rules.iter().enumerate() {
        let id = &rule.id;
        if id.trim().is_empty() {
            return Err(S3Error::Invalid("lifecycle rule: the ID is empty".to_owned()));
        }
        if id.chars().count() > RULE_ID_MAX {
            return Err(S3Error::Invalid(format!("lifecycle rule {id:?}: IDs are at most {RULE_ID_MAX} characters")));
        }
        if rules[..i].iter().any(|r| r.id == *id) {
            return Err(S3Error::Invalid(format!("lifecycle rule {id:?}: the ID is there twice")));
        }
        if rule.kept.is_some() {
            continue;
        }
        let days = [rule.expire_days, rule.noncurrent_days, rule.abort_days];
        if days.iter().all(Option::is_none) {
            return Err(S3Error::Invalid(format!("lifecycle rule {id:?}: it sets no number of days")));
        }
        if days.contains(&Some(0)) {
            return Err(S3Error::Invalid(format!("lifecycle rule {id:?}: days must be 1 or more")));
        }
    }
    Ok(())
}

/// Checks `rules` before they're sent: each with an origin and a method S3 knows.
pub fn check_cors(rules: &[CorsRule]) -> Result<(), S3Error> {
    if rules.len() > MAX_CORS_RULES {
        return Err(S3Error::Invalid(format!("CORS: a bucket has at most {MAX_CORS_RULES} rules")));
    }
    for (i, rule) in rules.iter().enumerate() {
        let n = i + 1;
        if rule.origins.iter().all(|o| o.trim().is_empty()) {
            return Err(S3Error::Invalid(format!("CORS rule {n}: it allows no origin")));
        }
        if rule.methods.is_empty() {
            return Err(S3Error::Invalid(format!("CORS rule {n}: it allows no method")));
        }
        if let Some(m) = rule.methods.iter().find(|m| !CORS_METHODS.contains(&m.as_str())) {
            return Err(S3Error::Invalid(format!("CORS rule {n}: {m:?} isn't a method S3 allows")));
        }
        if let Some(o) = rule.origins.iter().find(|o| o.matches('*').count() > 1) {
            return Err(S3Error::Invalid(format!("CORS rule {n}: origin {o:?} has more than one *")));
        }
    }
    Ok(())
}

impl Session {
    /// Bucket `bucket`'s tags, in the order the gateway sent them; none when it has none.
    pub fn bucket_tags(&self, bucket: &str) -> Pending<Vec<BucketTag>> {
        let bucket = bucket.to_owned();
        self.spawn(|client| async move {
            match client.get_bucket_tagging().bucket(&bucket).send().await.map_err(map) {
                Ok(out) => Ok(out.tag_set().iter().map(|t| (t.key().to_owned(), t.value().to_owned())).collect()),
                Err(e) if missing(&e) => Ok(Vec::new()),
                Err(e) => Err(e),
            }
        })
    }

    /// Replaces bucket `bucket`'s tags with `tags`; none removes them.
    pub fn set_bucket_tags(&self, bucket: &str, tags: Vec<BucketTag>) -> Pending<()> {
        let bucket = bucket.to_owned();
        self.spawn(|client| async move {
            check_tags(&tags)?;
            if tags.is_empty() {
                client.delete_bucket_tagging().bucket(&bucket).send().await.map_err(map)?;
                return Ok(());
            }
            let set = tags
                .into_iter()
                .map(|(k, v)| Tag::builder().key(k).value(v).build())
                .collect::<Result<Vec<_>, _>>()
                .map_err(internal)?;
            let tagging = Tagging::builder().set_tag_set(Some(set)).build().map_err(internal)?;
            client.put_bucket_tagging().bucket(&bucket).tagging(tagging).send().await.map_err(map)?;
            Ok(())
        })
    }

    /// Bucket `bucket`'s lifecycle rules; none when it has none.
    pub fn lifecycle(&self, bucket: &str) -> Pending<Vec<LifecycleRule>> {
        let bucket = bucket.to_owned();
        self.spawn(|client| async move {
            match client.get_bucket_lifecycle_configuration().bucket(&bucket).send().await.map_err(map) {
                Ok(out) => Ok(out.rules().iter().map(from_sdk).collect()),
                Err(e) if missing(&e) => Ok(Vec::new()),
                Err(e) => Err(e),
            }
        })
    }

    /// Replaces bucket `bucket`'s lifecycle rules with `rules`; none removes them.
    pub fn set_lifecycle(&self, bucket: &str, rules: Vec<LifecycleRule>) -> Pending<()> {
        let bucket = bucket.to_owned();
        self.spawn(|client| async move {
            check_lifecycle(&rules)?;
            if rules.is_empty() {
                client.delete_bucket_lifecycle().bucket(&bucket).send().await.map_err(map)?;
                return Ok(());
            }
            let rules = rules.iter().map(to_sdk).collect::<Result<Vec<_>, _>>()?;
            let config = BucketLifecycleConfiguration::builder().set_rules(Some(rules)).build().map_err(internal)?;
            client
                .put_bucket_lifecycle_configuration()
                .bucket(&bucket)
                .lifecycle_configuration(config)
                .send()
                .await
                .map_err(map)?;
            Ok(())
        })
    }

    /// Bucket `bucket`'s CORS rules; none when it has none.
    pub fn cors(&self, bucket: &str) -> Pending<Vec<CorsRule>> {
        let bucket = bucket.to_owned();
        self.spawn(|client| async move {
            match client.get_bucket_cors().bucket(&bucket).send().await.map_err(map) {
                Ok(out) => Ok(out
                    .cors_rules()
                    .iter()
                    .map(|r| CorsRule {
                        id: r.id().map(str::to_owned),
                        origins: r.allowed_origins().to_vec(),
                        methods: r.allowed_methods().to_vec(),
                        allowed_headers: r.allowed_headers().to_vec(),
                        expose_headers: r.expose_headers().to_vec(),
                        max_age: r.max_age_seconds().and_then(|s| u32::try_from(s).ok()),
                    })
                    .collect()),
                Err(e) if missing(&e) => Ok(Vec::new()),
                Err(e) => Err(e),
            }
        })
    }

    /// Replaces bucket `bucket`'s CORS rules with `rules`; none removes them.
    pub fn set_cors(&self, bucket: &str, rules: Vec<CorsRule>) -> Pending<()> {
        let bucket = bucket.to_owned();
        self.spawn(|client| async move {
            check_cors(&rules)?;
            if rules.is_empty() {
                client.delete_bucket_cors().bucket(&bucket).send().await.map_err(map)?;
                return Ok(());
            }
            let rules = rules
                .into_iter()
                .map(|r| {
                    aws_sdk_s3::types::CorsRule::builder()
                        .set_id(r.id)
                        .set_allowed_origins(Some(r.origins))
                        .set_allowed_methods(Some(r.methods))
                        .set_allowed_headers(Some(r.allowed_headers).filter(|h| !h.is_empty()))
                        .set_expose_headers(Some(r.expose_headers).filter(|h| !h.is_empty()))
                        .set_max_age_seconds(r.max_age.map(|s| i32::try_from(s).unwrap_or(i32::MAX)))
                        .build()
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(internal)?;
            let config = CorsConfiguration::builder().set_cors_rules(Some(rules)).build().map_err(internal)?;
            client.put_bucket_cors().bucket(&bucket).cors_configuration(config).send().await.map_err(map)?;
            Ok(())
        })
    }
}

/// Whether `error` says the bucket has no such configuration, which reads as an empty one.
fn missing(error: &S3Error) -> bool {
    matches!(error, S3Error::Service { code, .. }
        if matches!(code.as_str(), "NoSuchTagSet" | "NoSuchTagSetError" | "NoSuchLifecycleConfiguration"
            | "NoSuchCORSConfiguration"))
}

/// A request the SDK couldn't build: a bug, as every field is checked first.
#[expect(clippy::needless_pass_by_value, reason = "taken by value to be passed as `map_err(internal)`")]
fn internal(e: aws_sdk_s3::error::BuildError) -> S3Error {
    S3Error::Internal(e.to_string())
}

/// A count of days from the SDK, which gives them signed.
fn days(d: Option<i32>) -> Option<u32> {
    d.and_then(|d| u32::try_from(d).ok())
}

/// The window's view of SDK rule `rule`.
fn from_sdk(rule: &aws_sdk_s3::types::LifecycleRule) -> LifecycleRule {
    #[expect(deprecated, reason = "the rule-level prefix is how older clients wrote the filter")]
    let legacy = rule.prefix();
    let filter = rule.filter();
    let expiration = rule.expiration();
    let noncurrent = rule.noncurrent_version_expiration();
    let mut extras = Vec::new();
    if filter.is_some_and(|f| f.tag().is_some() || f.and().is_some()) {
        extras.push("a tag filter");
    }
    if filter.is_some_and(|f| f.object_size_greater_than().is_some() || f.object_size_less_than().is_some()) {
        extras.push("a size filter");
    }
    if expiration.is_some_and(|e| e.date().is_some()) {
        extras.push("an expiry date");
    }
    if expiration.is_some_and(|e| e.expired_object_delete_marker().is_some()) {
        extras.push("delete-marker cleanup");
    }
    if !rule.transitions().is_empty() || !rule.noncurrent_version_transitions().is_empty() {
        extras.push("transitions");
    }
    if noncurrent.is_some_and(|n| n.newer_noncurrent_versions().is_some()) {
        extras.push("versions to keep");
    }
    LifecycleRule {
        id: rule.id().unwrap_or_default().to_owned(),
        prefix: filter.and_then(LifecycleRuleFilter::prefix).or(legacy).unwrap_or_default().to_owned(),
        enabled: *rule.status() == ExpirationStatus::Enabled,
        expire_days: days(expiration.and_then(LifecycleExpiration::days)),
        noncurrent_days: days(noncurrent.and_then(NoncurrentVersionExpiration::noncurrent_days)),
        abort_days: days(
            rule.abort_incomplete_multipart_upload().and_then(AbortIncompleteMultipartUpload::days_after_initiation),
        ),
        kept: (!extras.is_empty()).then(|| KeptRule { rule: rule.clone(), extras }),
    }
}

/// The SDK rule for `rule`: the kept one as it came, else one built from its fields.
fn to_sdk(rule: &LifecycleRule) -> Result<aws_sdk_s3::types::LifecycleRule, S3Error> {
    if let Some(kept) = &rule.kept {
        return Ok(kept.rule.clone());
    }
    let signed = |d: u32| i32::try_from(d).unwrap_or(i32::MAX);
    aws_sdk_s3::types::LifecycleRule::builder()
        .id(&rule.id)
        .filter(LifecycleRuleFilter::builder().prefix(&rule.prefix).build())
        .status(if rule.enabled { ExpirationStatus::Enabled } else { ExpirationStatus::Disabled })
        .set_expiration(rule.expire_days.map(|d| LifecycleExpiration::builder().days(signed(d)).build()))
        .set_noncurrent_version_expiration(
            rule.noncurrent_days.map(|d| NoncurrentVersionExpiration::builder().noncurrent_days(signed(d)).build()),
        )
        .set_abort_incomplete_multipart_upload(
            rule.abort_days.map(|d| AbortIncompleteMultipartUpload::builder().days_after_initiation(signed(d)).build()),
        )
        .build()
        .map_err(internal)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(k: &str, v: &str) -> BucketTag {
        (k.to_owned(), v.to_owned())
    }

    fn rule(id: &str, expire: Option<u32>) -> LifecycleRule {
        LifecycleRule { expire_days: expire, ..LifecycleRule::new(id) }
    }

    fn cors(origins: &[&str], methods: &[&str]) -> CorsRule {
        CorsRule {
            origins: origins.iter().map(|s| (*s).to_owned()).collect(),
            methods: methods.iter().map(|s| (*s).to_owned()).collect(),
            ..CorsRule::default()
        }
    }

    fn invalid(result: Result<(), S3Error>) -> String {
        match result {
            Err(S3Error::Invalid(why)) => why,
            other => panic!("not refused as invalid: {other:?}"),
        }
    }

    #[test]
    fn tags_need_unique_keys_within_s3s_limits() {
        assert!(check_tags(&[]).is_ok());
        assert!(check_tags(&[tag("team", "a"), tag("env", "")]).is_ok(), "an empty value is a value");
        assert!(invalid(check_tags(&[tag(" ", "x")])).contains("empty"));
        assert!(invalid(check_tags(&[tag("team", "a"), tag("team", "b")])).contains("twice"));
        assert!(invalid(check_tags(&[tag("aws:x", "a")])).contains("reserved"));
        assert!(check_tags(&[tag(&"k".repeat(128), &"v".repeat(256))]).is_ok());
        assert!(invalid(check_tags(&[tag(&"k".repeat(129), "")])).contains("128"));
        assert!(invalid(check_tags(&[tag("k", &"v".repeat(257))])).contains("256"));
        let many: Vec<_> = (0..=MAX_TAGS).map(|i| tag(&format!("k{i}"), "")).collect();
        assert!(check_tags(&many[..MAX_TAGS]).is_ok());
        assert!(invalid(check_tags(&many)).contains("at most 50"));
    }

    #[test]
    fn lifecycle_rules_need_an_id_and_some_days() {
        assert!(check_lifecycle(&[rule("logs", Some(30))]).is_ok());
        assert!(invalid(check_lifecycle(&[rule("", Some(30))])).contains("ID is empty"));
        assert!(invalid(check_lifecycle(&[rule("a", Some(1)), rule("a", Some(2))])).contains("twice"));
        assert!(invalid(check_lifecycle(&[rule("a", None)])).contains("no number of days"));
        assert!(invalid(check_lifecycle(&[rule("a", Some(0))])).contains("1 or more"));
        let aborts = LifecycleRule { abort_days: Some(7), ..LifecycleRule::new("uploads") };
        let noncurrent = LifecycleRule { noncurrent_days: Some(7), ..LifecycleRule::new("old") };
        assert!(check_lifecycle(&[aborts, noncurrent]).is_ok());
        assert!(invalid(check_lifecycle(&[rule(&"i".repeat(256), Some(1))])).contains("255"));
    }

    #[test]
    fn a_rule_the_window_cant_edit_is_kept_as_it_came() {
        let sdk = aws_sdk_s3::types::LifecycleRule::builder()
            .id("archive")
            .filter(LifecycleRuleFilter::builder().prefix("logs/").object_size_greater_than(1024).build())
            .status(ExpirationStatus::Enabled)
            .expiration(LifecycleExpiration::builder().days(90).build())
            .build()
            .unwrap();
        let read = from_sdk(&sdk);
        assert_eq!((read.id.as_str(), read.prefix.as_str(), read.expire_days), ("archive", "logs/", Some(90)));
        assert_eq!(read.kept.as_ref().map(|k| k.extras.clone()), Some(vec!["a size filter"]));
        assert_eq!(to_sdk(&read).unwrap(), sdk, "saved back unchanged");
        // Kept rules aren't checked for days: the gateway took them as they are.
        assert!(check_lifecycle(&[LifecycleRule { expire_days: None, ..read }]).is_ok());
    }

    #[test]
    fn an_editable_rule_goes_there_and_back() {
        let rule = LifecycleRule {
            id: "tidy".to_owned(),
            prefix: "tmp/".to_owned(),
            enabled: false,
            expire_days: Some(30),
            noncurrent_days: Some(7),
            abort_days: Some(2),
            kept: None,
        };
        assert_eq!(from_sdk(&to_sdk(&rule).unwrap()), rule);
        let whole_bucket = LifecycleRule { prefix: String::new(), ..rule };
        assert_eq!(from_sdk(&to_sdk(&whole_bucket).unwrap()), whole_bucket);
    }

    #[test]
    fn a_legacy_prefix_is_read_as_the_filter() {
        #[expect(deprecated, reason = "what older clients write")]
        let sdk = aws_sdk_s3::types::LifecycleRule::builder()
            .id("old")
            .prefix("cache/")
            .status(ExpirationStatus::Enabled)
            .expiration(LifecycleExpiration::builder().days(1).build())
            .build()
            .unwrap();
        let read = from_sdk(&sdk);
        assert_eq!((read.prefix.as_str(), read.kept.is_none()), ("cache/", true));
    }

    #[test]
    fn every_extra_is_named() {
        let sdk = aws_sdk_s3::types::LifecycleRule::builder()
            .id("all")
            .filter(LifecycleRuleFilter::builder().tag(Tag::builder().key("k").value("v").build().unwrap()).build())
            .status(ExpirationStatus::Enabled)
            .expiration(
                LifecycleExpiration::builder()
                    .date(aws_smithy_types::DateTime::from_secs(0))
                    .expired_object_delete_marker(true)
                    .build(),
            )
            .noncurrent_version_expiration(
                NoncurrentVersionExpiration::builder().noncurrent_days(1).newer_noncurrent_versions(3).build(),
            )
            .transitions(aws_sdk_s3::types::Transition::builder().days(1).build())
            .build()
            .unwrap();
        let extras = from_sdk(&sdk).kept.unwrap().extras;
        assert_eq!(
            extras,
            ["a tag filter", "an expiry date", "delete-marker cleanup", "transitions", "versions to keep"]
        );
    }

    #[test]
    fn cors_rules_need_an_origin_and_known_methods() {
        assert!(check_cors(&[cors(&["https://app.example.com"], &["GET", "HEAD"])]).is_ok());
        assert!(check_cors(&[cors(&["*"], &["PUT"])]).is_ok());
        assert!(check_cors(&[cors(&["https://*.example.com"], &["GET"])]).is_ok());
        assert!(invalid(check_cors(&[cors(&[], &["GET"])])).contains("no origin"));
        assert!(invalid(check_cors(&[cors(&[" "], &["GET"])])).contains("no origin"));
        assert!(invalid(check_cors(&[cors(&["*"], &[])])).contains("no method"));
        assert!(invalid(check_cors(&[cors(&["*"], &["PATCH"])])).contains("PATCH"));
        assert!(
            invalid(check_cors(&[cors(&["*"], &["GET"]), cors(&["https://*.*.com"], &["GET"])]))
                .starts_with("CORS rule 2")
        );
    }

    #[test]
    fn only_a_missing_configuration_reads_as_empty() {
        let service = |code: &str| S3Error::Service { code: code.to_owned(), message: String::new() };
        for code in ["NoSuchTagSet", "NoSuchTagSetError", "NoSuchLifecycleConfiguration", "NoSuchCORSConfiguration"] {
            assert!(missing(&service(code)), "{code}");
        }
        assert!(!missing(&service("InternalError")));
        assert!(!missing(&S3Error::NotFound("NoSuchBucket".to_owned())));
        assert!(!missing(&S3Error::AccessDenied(String::new())));
    }
}
