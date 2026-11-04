//! What the profile may do in its account, read once from its own IAM policies so the window shows only that: an
//! account's users usually work under group policies (objects read-write, buckets read-only or read-write, and only
//! buckets named `PROJECT-*-ENV`). Reading them needs the admins to let each user read their own (radosgw's account
//! users start with no permissions, IAM's included); without that it's [`Access::Unknown`] and the window asks the
//! gateway as it goes. See `docs/decisions/0004-*`.

use super::error::map;
use super::policy::{self, Decision, Statement};
use super::{Pending, S3Error, Session};

/// `AmazonS3FullAccess`, one of the AWS managed policies radosgw supports.
const S3_FULL: &str = r#"{"Statement":[{"Effect":"Allow","Action":["s3:*","s3-object-lambda:*"],"Resource":"*"}]}"#;

/// `AmazonS3ReadOnlyAccess`.
const S3_READ_ONLY: &str =
    r#"{"Statement":[{"Effect":"Allow","Action":["s3:Get*","s3:List*","s3:Describe*"],"Resource":"*"}]}"#;

/// The account root's own rights: everything in the account, before the explicit Denies its policies add.
const ROOT: &str = r#"{"Statement":[{"Effect":"Allow","Action":"*","Resource":"*"}]}"#;

/// What the profile may do.
#[derive(Debug, Clone)]
pub enum Access {
    /// The account's root user: everything in the account but what an explicit Deny in its own policies refuses
    /// (the platform team's guardrails, `docs/decisions/0005-*`).
    Root {
        /// [`ROOT`]'s statement and those of the root's policies.
        statements: Vec<Statement>,
    },
    /// An IAM user of the account, with the policies of its groups and its own.
    Policies {
        /// The user's name.
        user: String,
        /// Its groups, by name.
        groups: Vec<String>,
        /// Every statement of every policy it has.
        statements: Vec<Statement>,
    },
    /// Not known, with why (the policies can't be read, or the gateway has no IAM): the gateway decides each request.
    Unknown(String),
}

impl Access {
    /// What the policies say about `action` (e.g. `s3:PutObject`) on `bucket`, or on object `key` in it.
    pub fn decide(&self, action: &str, bucket: &str, key: Option<&str>) -> Decision {
        match self {
            Self::Unknown(_) => Decision::Unknown,
            Self::Root { statements } | Self::Policies { statements, .. } => {
                let resource = match key {
                    Some(key) => format!("arn:aws:s3:::{bucket}/{key}"),
                    None => format!("arn:aws:s3:::{bucket}"),
                };
                policy::decide(statements, action, &resource)
            }
        }
    }

    /// Whether `action` may be offered: anything not surely denied, so an unknown answer leaves it to the gateway.
    pub fn may(&self, action: &str, bucket: &str, key: Option<&str>) -> bool {
        self.decide(action, bucket, key) != Decision::Denied
    }

    /// The bucket names the profile may create, as wildcard patterns (`r3manager-*-dev`); empty when it may create
    /// none. `None` when any name may be tried: the root user, or policies not known.
    pub fn bucket_patterns(&self) -> Option<Vec<String>> {
        let Self::Policies { statements, .. } = self else { return None };
        let patterns = policy::allowed_resources(statements, "s3:CreateBucket");
        if patterns.iter().any(|p| p == "*" || p == "arn:aws:s3:::*") {
            return None;
        }
        Some(patterns.into_iter().filter_map(|p| p.strip_prefix("arn:aws:s3:::").map(str::to_owned)).collect())
    }
}

impl Session {
    /// Reads what the profile may do from its own IAM identity and policies: its groups' inline and attached
    /// policies and its own. Never fails: what can't be read is [`Access::Unknown`].
    pub fn access(&self) -> Pending<Access> {
        let iam = self.iam.clone();
        self.spawn(move |_| async move {
            Ok(match load(&iam).await {
                Ok(access) => access,
                Err(S3Error::AccessDenied(_)) => Access::Unknown("this user may not read its own policies".to_owned()),
                Err(e) => Access::Unknown(e.to_string()),
            })
        })
    }
}

/// [`Session::access`]'s reads. Lists aren't paged: IAM returns 100 entries a page, more groups or policies than any
/// one user has.
async fn load(iam: &aws_sdk_iam::Client) -> Result<Access, S3Error> {
    let me = iam.get_user().send().await.map_err(map)?;
    let user = me.user().ok_or_else(|| S3Error::Internal("GetUser named no user".to_owned()))?;
    let name = user.user_name().to_owned();
    let mut documents = Vec::new();
    for policy in iam.list_user_policies().user_name(&name).send().await.map_err(map)?.policy_names() {
        let doc = iam.get_user_policy().user_name(&name).policy_name(policy).send().await.map_err(map)?;
        documents.push(doc.policy_document().to_owned());
    }
    let attached = iam.list_attached_user_policies().user_name(&name).send().await.map_err(map)?;
    documents.extend(attached.attached_policies().iter().filter_map(|p| managed(p.policy_arn()?)));
    // The account's root user is named by the account's own ARN, `arn:aws:iam::ACCOUNT:root`; it has no groups.
    if user.arn().ends_with(":root") {
        documents.push(ROOT.to_owned());
        return Ok(match parse_all(&documents) {
            Ok(statements) => Access::Root { statements },
            Err(e) => Access::Unknown(format!("a policy of the account root couldn't be read: {e}")),
        });
    }
    let mut groups = Vec::new();
    for group in iam.list_groups_for_user().user_name(&name).send().await.map_err(map)?.groups() {
        let group = group.group_name().to_owned();
        for policy in iam.list_group_policies().group_name(&group).send().await.map_err(map)?.policy_names() {
            let doc = iam.get_group_policy().group_name(&group).policy_name(policy).send().await.map_err(map)?;
            documents.push(doc.policy_document().to_owned());
        }
        let attached = iam.list_attached_group_policies().group_name(&group).send().await.map_err(map)?;
        documents.extend(attached.attached_policies().iter().filter_map(|p| managed(p.policy_arn()?)));
        groups.push(group);
    }
    let statements = match parse_all(&documents) {
        Ok(statements) => statements,
        Err(e) => return Ok(Access::Unknown(format!("a policy of {name} couldn't be read: {e}"))),
    };
    tracing::info!(target: "r3manager::s3", user = %name, groups = ?groups, statements = statements.len(), "policies loaded");
    Ok(Access::Policies { user: name, groups, statements })
}

/// Every statement of `documents`. One unreadable policy fails them all, as it would make every answer a guess.
fn parse_all(documents: &[String]) -> Result<Vec<Statement>, String> {
    documents.iter().try_fold(Vec::new(), |mut all, doc| {
        all.extend(policy::parse(doc)?);
        Ok(all)
    })
}

/// The document of AWS managed policy `arn` when it grants S3 rights; the others radosgw supports (IAM, SNS) grant
/// none.
fn managed(arn: &str) -> Option<String> {
    match arn.rsplit('/').next()? {
        "AmazonS3FullAccess" => Some(S3_FULL.to_owned()),
        "AmazonS3ReadOnlyAccess" => Some(S3_READ_ONLY.to_owned()),
        _ => None,
    }
}
