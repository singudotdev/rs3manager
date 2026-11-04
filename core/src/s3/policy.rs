//! IAM identity policies, as far as the window needs them: which S3 actions on which buckets and keys a user's
//! policies allow, to show only what they may do. The gateway stays the authority; a wrong answer here only shows or
//! hides a button.
//!
//! Evaluation follows [AWS's policy evaluation logic] for identity policies within one account: an explicit Deny wins,
//! else an Allow grants, else it's denied. Statements with a `Condition` or a policy variable can't be judged without
//! the request, so they make the answer [`Decision::Unknown`] when they'd change it. Bucket policies aren't read
//! (a user can't read them before having access), so something a bucket policy alone grants shows as denied.
//!
//! [AWS's policy evaluation logic]: https://docs.aws.amazon.com/IAM/latest/UserGuide/reference_policies_evaluation-logic.html

use serde_json::Value;

/// What the policies say about one request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// An Allow applies and no Deny does.
    Allowed,
    /// A Deny applies, or no Allow does.
    Denied,
    /// It depends on a condition or variable only the gateway can evaluate.
    Unknown,
}

/// The actions or resources a statement names: listed (`Action`, `Resource`) or all but those listed (`NotAction`,
/// `NotResource`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum Patterns {
    /// Matches any of these.
    Only(Vec<String>),
    /// Matches anything but these.
    Except(Vec<String>),
}

impl Patterns {
    /// Whether `value` is matched, comparing without case when `fold` (actions are case-insensitive).
    fn matches(&self, value: &str, fold: bool) -> bool {
        let any = |list: &[String]| list.iter().any(|p| glob(p, value, fold));
        match self {
            Self::Only(list) => any(list),
            Self::Except(list) => !any(list),
        }
    }
}

/// One statement of an identity policy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Statement {
    /// `Effect: Allow` (else Deny).
    allow: bool,
    /// Its actions.
    actions: Patterns,
    /// Its resources.
    resources: Patterns,
    /// Whether it has a `Condition` or a policy variable, so whether it applies depends on the request.
    conditional: bool,
}

/// The statements of policy document `doc`. IAM's API returns documents URL-encoded; both forms are taken.
pub fn parse(doc: &str) -> Result<Vec<Statement>, String> {
    let json: Value = serde_json::from_str(doc)
        .or_else(|_| serde_json::from_str(&percent_decode(doc)))
        .map_err(|e| format!("not a JSON policy: {e}"))?;
    let statements = match json.get("Statement") {
        Some(Value::Array(list)) => list.iter().collect(),
        Some(one @ Value::Object(_)) => vec![one],
        _ => return Err("no Statement".to_owned()),
    };
    statements.into_iter().map(statement).collect()
}

/// One statement from its JSON.
fn statement(json: &Value) -> Result<Statement, String> {
    let allow = match json.get("Effect").and_then(Value::as_str) {
        Some("Allow") => true,
        Some("Deny") => false,
        other => return Err(format!("Effect {other:?}")),
    };
    let patterns = |name: &str, not: &str| match (json.get(name), json.get(not)) {
        (Some(v), None) => strings(v).map(Patterns::Only),
        (None, Some(v)) => strings(v).map(Patterns::Except),
        _ => Err(format!("a statement needs {name} or {not}")),
    };
    let (actions, mut resources) = (patterns("Action", "NotAction")?, patterns("Resource", "NotResource")?);
    // A policy variable (`${aws:username}`) is only known to the gateway: it matches anything here, and the statement
    // counts as conditional, so it can only make an answer unknown.
    let mut variable = false;
    let (Patterns::Only(list) | Patterns::Except(list)) = &mut resources;
    for resource in list.iter_mut().filter(|r| r.contains("${")) {
        variable = true;
        *resource = without_variables(resource);
    }
    Ok(Statement { allow, actions, resources, conditional: json.get("Condition").is_some() || variable })
}

/// `pattern` with each `${...}` variable replaced by `*`.
fn without_variables(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut rest = pattern;
    while let Some(start) = rest.find("${") {
        out.push_str(&rest[..start]);
        out.push('*');
        rest = rest[start..].find('}').map_or("", |end| &rest[start + end + 1..]);
    }
    out.push_str(rest);
    out
}

/// A string or a list of them.
fn strings(value: &Value) -> Result<Vec<String>, String> {
    match value {
        Value::String(s) => Ok(vec![s.clone()]),
        Value::Array(list) => {
            list.iter().map(|v| v.as_str().map(str::to_owned).ok_or_else(|| format!("not a string: {v}"))).collect()
        }
        other => Err(format!("not a string or list: {other}")),
    }
}

/// What `statements` say about `action` (e.g. `s3:PutObject`) on `resource` (an ARN).
pub fn decide(statements: &[Statement], action: &str, resource: &str) -> Decision {
    let applying =
        statements.iter().filter(|s| s.actions.matches(action, true) && s.resources.matches(resource, false));
    let (mut allowed, mut maybe) = (false, false);
    for s in applying {
        match (s.allow, s.conditional) {
            (false, false) => return Decision::Denied,
            (true, false) => allowed = true,
            (_, true) => maybe = true,
        }
    }
    match (allowed, maybe) {
        (true, false) => Decision::Allowed,
        (false, false) => Decision::Denied,
        (_, true) => Decision::Unknown,
    }
}

/// The resources unconditional Allow statements grant `action` on, as written: for bucket names, the patterns a
/// user may create (`arn:aws:s3:::r3manager-*-dev`). A Deny on some of them isn't subtracted; [`decide`] still
/// judges each name.
pub fn allowed_resources(statements: &[Statement], action: &str) -> Vec<String> {
    statements
        .iter()
        .filter(|s| s.allow && !s.conditional && s.actions.matches(action, true))
        .filter_map(|s| match &s.resources {
            Patterns::Only(list) => Some(list.clone()),
            Patterns::Except(_) => None,
        })
        .flatten()
        .collect()
}

/// Whether `value` matches IAM wildcard `pattern`: `*` any run of characters, `?` any one.
fn glob(pattern: &str, value: &str, fold: bool) -> bool {
    let fold_char = |c: char| if fold { c.to_ascii_lowercase() } else { c };
    let (p, v): (Vec<char>, Vec<char>) =
        (pattern.chars().map(fold_char).collect(), value.chars().map(fold_char).collect());
    // Iterative matching with one backtrack point, linear in practice and with no recursion on hostile input.
    let (mut pi, mut vi, mut star, mut mark) = (0, 0, None, 0);
    while vi < v.len() {
        if pi < p.len() && (p[pi] == '?' || p[pi] == v[vi]) {
            pi += 1;
            vi += 1;
        } else if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = vi;
            pi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            vi = mark;
        } else {
            return false;
        }
    }
    p[pi..].iter().all(|&c| c == '*')
}

/// `text` with its `%XX` escapes decoded, as IAM returns policy documents.
fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' if i + 2 < bytes.len() => {
                match u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16) {
                    Ok(b) => {
                        out.push(b);
                        i += 3;
                        continue;
                    }
                    Err(_) => out.push(b'%'),
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fmt::Write as _;

    /// The team's groups for buckets `r3manager-*-ENV`, as `dev/rgw.sh project` writes them.
    fn team(env: &str, buckets_rw: bool) -> Vec<Statement> {
        let objects = format!(
            r#"{{"Version":"2012-10-17","Statement":[{{"Effect":"Allow","Action":["s3:GetObject","s3:PutObject",
            "s3:DeleteObject"],"Resource":"arn:aws:s3:::r3manager-*-{env}/*"}}]}}"#
        );
        let mut buckets = format!(
            r#"{{"Effect":"Allow","Action":"s3:ListAllMyBuckets","Resource":"*"}},{{"Effect":"Allow",
            "Action":["s3:ListBucket"],"Resource":"arn:aws:s3:::r3manager-*-{env}"}}"#
        );
        if buckets_rw {
            let _ = write!(
                buckets,
                r#",{{"Effect":"Allow","Action":["s3:CreateBucket","s3:DeleteBucket"],"Resource":"arn:aws:s3:::r3manager-*-{env}"}}"#
            );
        }
        let mut all = parse(&objects).unwrap();
        all.extend(parse(&format!(r#"{{"Version":"2012-10-17","Statement":[{buckets}]}}"#)).unwrap());
        all
    }

    #[test]
    fn the_team_groups_allow_objects_and_buckets_by_name() {
        let prod = team("prod", false);
        assert_eq!(decide(&prod, "s3:PutObject", "arn:aws:s3:::r3manager-web-prod/a/b.txt"), Decision::Allowed);
        assert_eq!(decide(&prod, "s3:ListBucket", "arn:aws:s3:::r3manager-web-prod"), Decision::Allowed);
        assert_eq!(decide(&prod, "s3:ListBucket", "arn:aws:s3:::legacy-prod"), Decision::Denied);
        assert_eq!(decide(&prod, "s3:CreateBucket", "arn:aws:s3:::r3manager-api-prod"), Decision::Denied);
        assert_eq!(allowed_resources(&prod, "s3:CreateBucket"), Vec::<String>::new());

        let dev = team("dev", true);
        assert_eq!(decide(&dev, "s3:CreateBucket", "arn:aws:s3:::r3manager-api-dev"), Decision::Allowed);
        assert_eq!(decide(&dev, "s3:CreateBucket", "arn:aws:s3:::otro-api-dev"), Decision::Denied);
        assert_eq!(decide(&dev, "s3:createbucket", "arn:aws:s3:::r3manager-api-dev"), Decision::Allowed, "any case");
        assert_eq!(allowed_resources(&dev, "s3:CreateBucket"), ["arn:aws:s3:::r3manager-*-dev"]);
    }

    #[test]
    fn deny_wins_and_conditions_are_left_to_the_gateway() {
        let doc = r#"{"Statement":[
            {"Effect":"Allow","Action":"s3:*","Resource":"*"},
            {"Effect":"Deny","Action":"s3:DeleteObject","Resource":"arn:aws:s3:::logs-*/*"},
            {"Effect":"Deny","NotAction":"s3:Get*","Resource":"arn:aws:s3:::archive/*"},
            {"Effect":"Allow","Action":"s3:PutObject","Resource":"arn:aws:s3:::home/${aws:username}/*"},
            {"Effect":"Allow","Action":"s3:GetObject","Resource":"arn:aws:s3:::vpn/*","Condition":{"IpAddress":{"aws:SourceIp":"10.0.0.0/8"}}}
        ]}"#;
        let s = parse(doc).unwrap();
        assert_eq!(decide(&s, "s3:DeleteObject", "arn:aws:s3:::logs-app/x"), Decision::Denied);
        assert_eq!(decide(&s, "s3:DeleteObject", "arn:aws:s3:::data/x"), Decision::Allowed);
        assert_eq!(decide(&s, "s3:PutObject", "arn:aws:s3:::archive/x"), Decision::Denied, "NotAction");
        assert_eq!(decide(&s, "s3:GetObject", "arn:aws:s3:::archive/x"), Decision::Allowed);
        let only_conditional =
            parse(&doc.replace(r#"{"Effect":"Allow","Action":"s3:*","Resource":"*"},"#, "")).unwrap();
        assert_eq!(decide(&only_conditional, "s3:GetObject", "arn:aws:s3:::vpn/a"), Decision::Unknown);
        assert_eq!(decide(&only_conditional, "s3:PutObject", "arn:aws:s3:::home/x/a"), Decision::Unknown);
        assert_eq!(decide(&only_conditional, "s3:PutObject", "arn:aws:s3:::vpn/a"), Decision::Denied);
    }

    #[test]
    fn documents_come_url_encoded_or_plain_with_one_statement_or_many() {
        let plain = r#"{"Statement":{"Effect":"Allow","Action":"s3:GetObject","Resource":"arn:aws:s3:::b/*"}}"#;
        let encoded = "%7B%22Statement%22%3A%7B%22Effect%22%3A%22Allow%22%2C%22Action%22%3A%22s3%3AGetObject%22%2C%22Resource%22%3A%22arn%3Aaws%3As3%3A%3A%3Ab%2F%2A%22%7D%7D";
        assert_eq!(parse(plain).unwrap(), parse(encoded).unwrap());
        assert!(parse(r#"{"Statement":[{"Effect":"Maybe","Action":"*","Resource":"*"}]}"#).is_err());
    }

    #[test]
    fn wildcards_match_runs_and_single_characters() {
        assert!(glob("r3manager-*-dev", "r3manager-web-api-dev", false));
        assert!(!glob("r3manager-*-dev", "r3manager-web-prod", false));
        assert!(glob("a?c", "abc", false) && !glob("a?c", "ac", false));
        assert!(glob("*", "", false) && glob("S3:Get*", "s3:getobject", true));
        assert!(!glob("arn:aws:s3:::b", "arn:aws:s3:::b/x", false));
    }
}
