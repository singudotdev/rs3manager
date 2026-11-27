//! A bucket's settings as the dialog's fields hold them, plain text, and how that text becomes the core's tags,
//! lifecycle rules and CORS rules, or why it can't (decision #56).

use rs3manager_core::s3::settings::CORS_METHODS;
use rs3manager_core::s3::{BucketTag, CorsRule, LifecycleRule};

/// The dialog's tabs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    /// The bucket's tags.
    Tags,
    /// Its lifecycle rules.
    Lifecycle,
    /// Its CORS rules.
    Cors,
    /// Whether it keeps versions.
    Versioning,
}

impl Tab {
    /// Every tab, in the order shown.
    pub const ALL: [Self; 4] = [Self::Tags, Self::Lifecycle, Self::Cors, Self::Versioning];

    /// Its title.
    pub fn label(self) -> &'static str {
        match self {
            Self::Tags => "Tags",
            Self::Lifecycle => "Lifecycle",
            Self::Cors => "CORS",
            Self::Versioning => "Versioning",
        }
    }

    /// The IAM action reading it, which shows the tab.
    pub fn read_action(self) -> &'static str {
        match self {
            Self::Tags => "s3:GetBucketTagging",
            Self::Lifecycle => "s3:GetLifecycleConfiguration",
            Self::Cors => "s3:GetBucketCORS",
            Self::Versioning => "s3:GetBucketVersioning",
        }
    }

    /// The IAM action writing it, and removing it too, as in AWS.
    pub fn write_action(self) -> &'static str {
        match self {
            Self::Tags => "s3:PutBucketTagging",
            Self::Lifecycle => "s3:PutLifecycleConfiguration",
            Self::Cors => "s3:PutBucketCORS",
            Self::Versioning => "s3:PutBucketVersioning",
        }
    }

    /// What it holds, for messages: "the tags of …".
    pub fn noun(self) -> &'static str {
        match self {
            Self::Tags => "tags",
            Self::Lifecycle => "lifecycle rules",
            Self::Cors => "CORS rules",
            Self::Versioning => "versioning",
        }
    }
}

/// The tags in rows of key and value: trimmed keys, rows left wholly empty dropped.
pub fn tags(rows: &[(String, String)]) -> Vec<BucketTag> {
    rows.iter()
        .filter(|(k, v)| !(k.trim().is_empty() && v.trim().is_empty()))
        .map(|(k, v)| (k.trim().to_owned(), v.clone()))
        .collect()
}

/// A count of days typed in field `what`: none when empty.
pub fn days(text: &str, what: &str) -> Result<Option<u32>, String> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    text.parse().map(Some).map_err(|_| format!("{what}: {text:?} isn't a whole number of days"))
}

/// The items of a comma-separated list, trimmed, empty ones dropped.
pub fn list(text: &str) -> Vec<String> {
    text.split(',').map(str::trim).filter(|s| !s.is_empty()).map(str::to_owned).collect()
}

/// `items` as a comma-separated list, as a field shows them.
pub fn joined(items: &[String]) -> String {
    items.join(", ")
}

/// A count of days as a field shows it: empty when none.
pub fn shown(days: Option<u32>) -> String {
    days.map_or_default(|d| d.to_string())
}

/// An ID for a new lifecycle rule that none of `taken` has: `rule-1`, `rule-2`…
pub fn new_rule_id(taken: &[String]) -> String {
    // One more number than IDs taken: one of them is free.
    (1..=taken.len() + 1).map(|n| format!("rule-{n}")).find(|id| !taken.contains(id)).unwrap_or_default()
}

/// A lifecycle rule's editable fields, as typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LifecycleText {
    /// Its ID.
    pub id: String,
    /// The key prefix it applies to.
    pub prefix: String,
    /// Whether it runs.
    pub enabled: bool,
    /// Days until objects expire.
    pub expire: String,
    /// Days until noncurrent versions are deleted.
    pub noncurrent: String,
    /// Days until unfinished uploads are aborted.
    pub abort: String,
}

impl LifecycleText {
    /// The rule these fields describe; the core checks the rest (IDs, at least one count, none zero).
    pub fn rule(&self) -> Result<LifecycleRule, String> {
        let id = self.id.trim();
        let what = |field: &str| if id.is_empty() { field.to_owned() } else { format!("rule {id:?}, {field}") };
        Ok(LifecycleRule {
            prefix: self.prefix.trim_start().to_owned(),
            enabled: self.enabled,
            expire_days: days(&self.expire, &what("expire objects"))?,
            noncurrent_days: days(&self.noncurrent, &what("delete noncurrent versions"))?,
            abort_days: days(&self.abort, &what("abort uploads"))?,
            ..LifecycleRule::new(id)
        })
    }
}

/// A CORS rule's fields, as typed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CorsText {
    /// Its ID as read, kept: the window doesn't edit it.
    pub id: Option<String>,
    /// The origins, comma-separated.
    pub origins: String,
    /// Which of [`CORS_METHODS`] it allows.
    pub methods: [bool; CORS_METHODS.len()],
    /// The allowed request headers, comma-separated.
    pub allowed_headers: String,
    /// The exposed response headers, comma-separated.
    pub expose_headers: String,
    /// The preflight's cache time, in seconds.
    pub max_age: String,
}

impl CorsText {
    /// Fields holding `rule`; a method S3 doesn't know is dropped, as S3 wouldn't have stored it.
    pub fn of(rule: &CorsRule) -> Self {
        Self {
            id: rule.id.clone(),
            origins: joined(&rule.origins),
            methods: CORS_METHODS.map(|m| rule.methods.iter().any(|r| r.eq_ignore_ascii_case(m))),
            allowed_headers: joined(&rule.allowed_headers),
            expose_headers: joined(&rule.expose_headers),
            max_age: shown(rule.max_age),
        }
    }

    /// A new rule's fields: GET from any origin, as a website reading the bucket needs.
    pub fn new_rule() -> Self {
        let mut methods = [false; CORS_METHODS.len()];
        methods[0] = true;
        Self { origins: "*".to_owned(), methods, ..Self::default() }
    }

    /// The rule these fields describe, the `n`th; the core checks origins and methods.
    pub fn rule(&self, n: usize) -> Result<CorsRule, String> {
        let age = self.max_age.trim();
        let max_age = if age.is_empty() {
            None
        } else {
            Some(age.parse().map_err(|_| format!("CORS rule {n}: max age {age:?} isn't a whole number of seconds"))?)
        };
        Ok(CorsRule {
            id: self.id.clone(),
            origins: list(&self.origins),
            methods: CORS_METHODS
                .iter()
                .zip(self.methods)
                .filter(|(_, on)| *on)
                .map(|(m, _)| (*m).to_owned())
                .collect(),
            allowed_headers: list(&self.allowed_headers),
            expose_headers: list(&self.expose_headers),
            max_age,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_tag_rows_are_dropped_and_keys_trimmed() {
        let rows = [
            (" team ".to_owned(), "a b ".to_owned()),
            (String::new(), " ".to_owned()),
            (String::new(), "orphan".to_owned()),
        ];
        assert_eq!(tags(&rows), [("team".to_owned(), "a b ".to_owned()), (String::new(), "orphan".to_owned())]);
    }

    #[test]
    fn days_are_whole_numbers_or_nothing() {
        assert_eq!(days(" 7 ", "x"), Ok(Some(7)));
        assert_eq!(days("  ", "x"), Ok(None));
        assert!(days("-1", "expire").unwrap_err().starts_with("expire: \"-1\""));
        assert!(days("1.5", "x").is_err());
    }

    #[test]
    fn lists_split_on_commas() {
        assert_eq!(list(" a, b ,,c "), ["a", "b", "c"]);
        assert_eq!(list(" , "), Vec::<String>::new());
        assert_eq!(joined(&list("ETag,x-amz-request-id")), "ETag, x-amz-request-id");
    }

    #[test]
    fn new_rule_ids_are_free() {
        let ids = |ids: &[&str]| ids.iter().map(|&i| i.to_owned()).collect::<Vec<_>>();
        assert_eq!(new_rule_id(&ids(&["tmp"])), "rule-1");
        assert_eq!(new_rule_id(&ids(&["rule-1", "rule-3", "rule-2"])), "rule-4");
        assert_eq!(new_rule_id(&ids(&["rule-2"])), "rule-1");
    }

    #[test]
    fn lifecycle_fields_make_a_rule() {
        let text = LifecycleText {
            id: " tmp ".to_owned(),
            prefix: "tmp/".to_owned(),
            enabled: false,
            expire: "7".to_owned(),
            noncurrent: String::new(),
            abort: "2".to_owned(),
        };
        let rule = text.rule().unwrap();
        assert_eq!(
            (rule.id.as_str(), rule.prefix.as_str(), rule.enabled, rule.expire_days, rule.noncurrent_days),
            ("tmp", "tmp/", false, Some(7), None)
        );
        assert_eq!(rule.abort_days, Some(2));
        let wrong = LifecycleText { noncurrent: "soon".to_owned(), ..text };
        assert_eq!(
            wrong.rule().unwrap_err(),
            "rule \"tmp\", delete noncurrent versions: \"soon\" isn't a whole number of days"
        );
    }

    #[test]
    fn cors_fields_go_there_and_back() {
        let rule = CorsRule {
            id: Some("app".to_owned()),
            origins: vec!["https://a.example.com".to_owned(), "https://b.example.com".to_owned()],
            methods: vec!["GET".to_owned(), "HEAD".to_owned()],
            allowed_headers: vec!["*".to_owned()],
            expose_headers: vec!["ETag".to_owned()],
            max_age: Some(600),
        };
        let text = CorsText::of(&rule);
        assert_eq!(text.origins, "https://a.example.com, https://b.example.com");
        assert_eq!(text.methods, [true, false, false, false, true]);
        assert_eq!(text.rule(1).unwrap(), rule);
        let wrong = CorsText { max_age: "an hour".to_owned(), ..text };
        assert!(wrong.rule(2).unwrap_err().starts_with("CORS rule 2: max age"));
        assert_eq!(CorsText::new_rule().rule(1).unwrap().methods, ["GET"]);
    }

    #[test]
    fn every_tab_names_its_iam_actions() {
        let actions: Vec<_> = Tab::ALL.iter().flat_map(|t| [t.read_action(), t.write_action()]).collect();
        assert_eq!(
            actions,
            [
                "s3:GetBucketTagging",
                "s3:PutBucketTagging",
                "s3:GetLifecycleConfiguration",
                "s3:PutLifecycleConfiguration",
                "s3:GetBucketCORS",
                "s3:PutBucketCORS",
                "s3:GetBucketVersioning",
                "s3:PutBucketVersioning",
            ]
        );
    }
}
