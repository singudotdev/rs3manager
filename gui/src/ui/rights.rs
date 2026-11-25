//! What the profile may do, loaded with the session and on Refresh, and the window's questions to it: which buckets to
//! show and which actions to offer. Something unknown is offered; the gateway decides.

use super::root::Root;
use crate::state::browser::Row;
use gpui::Context;
use rs3manager_core::s3::Access;

/// Reading an object: download, preview, share.
pub const READ: &[&str] = &["s3:GetObject"];
/// Deleting objects.
pub const DELETE: &[&str] = &["s3:DeleteObject"];
/// Renaming or moving: a copy, then a delete.
pub const RENAME: &[&str] = &["s3:GetObject", "s3:PutObject", "s3:DeleteObject"];
/// Changing an object's headers: a copy onto itself.
pub const EDIT_HEADERS: &[&str] = &["s3:GetObject", "s3:PutObject"];

/// The key standing for "a new object in this folder" when asking whether one may be written there.
const NEW_OBJECT: &str = "new-object";

impl Root {
    /// Reads the profile's policies, then lists the buckets it may see: by the policies when known, else by trying
    /// each, so buckets out of reach aren't shown either way.
    pub fn load_access(&mut self, cx: &mut Context<Self>) {
        self.buckets_loading = true;
        self.run(cx, rs3manager_core::s3::Session::access, |this, result, cx| {
            // `access` never fails; an error here is the session's own (cancelled).
            this.access = result.unwrap_or_else(|e| Access::Unknown(e.to_string()));
            if let Access::Unknown(why) = &this.access {
                tracing::info!(target: "rs3manager::ui", reason = %why, "permissions unknown, asking the gateway");
            }
            this.load_buckets(cx);
        });
    }

    /// Whether the policies may allow `action` in the open bucket, on `key` (or the bucket itself).
    pub fn may(&self, action: &str, key: Option<&str>) -> bool {
        self.browser.bucket().is_some_and(|bucket| self.access.may(action, bucket, key))
    }

    /// Whether a new object may be written in the open folder: uploads, new folders.
    pub fn may_write_here(&self) -> bool {
        self.may("s3:PutObject", Some(&format!("{}{NEW_OBJECT}", self.browser.prefix())))
    }

    /// Whether every one of `actions` may be done on each selected row (a folder's objects stand for themselves by
    /// a new key under it).
    pub fn may_on_selection(&self, actions: &[&str]) -> bool {
        let selection = self.browser.selection();
        !selection.is_empty()
            && selection.iter().all(|row| {
                let key = match row {
                    Row::Object(o) => o.key.clone(),
                    Row::Folder(prefix) => format!("{prefix}{NEW_OBJECT}"),
                };
                actions.iter().all(|a| self.may(a, Some(&key)))
            })
    }

    /// Whether every one of `actions` may be done on object `key` of the open bucket.
    pub fn may_on_key(&self, actions: &[&str], key: &str) -> bool {
        actions.iter().all(|a| self.may(a, Some(key)))
    }

    /// Whether bucket `name` may be deleted, emptied first when `force`.
    pub fn may_delete_bucket(&self, name: &str, force: bool) -> bool {
        self.access.may("s3:DeleteBucket", name, None)
            && (!force || self.access.may("s3:DeleteObject", name, Some(NEW_OBJECT)))
    }

    /// Whether some bucket may be created: not when the policies allow no name.
    pub fn may_create_buckets(&self) -> bool {
        self.access.bucket_patterns().is_none_or(|patterns| !patterns.is_empty())
    }

    /// Who the profile is, for the top bar: its IAM user and groups, the account's root, or that it's unknown.
    pub fn identity(&self) -> Option<String> {
        self.session.as_ref()?;
        Some(match &self.access {
            Access::Root { .. } => "account root".to_owned(),
            Access::Policies { user, groups, .. } if groups.is_empty() => user.clone(),
            Access::Policies { user, groups, .. } => format!("{user} · {}", groups.join(", ")),
            Access::Unknown(_) => "permissions not readable".to_owned(),
        })
    }
}
