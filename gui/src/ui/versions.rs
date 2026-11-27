//! The selected object's versions (decision #57): listed in the details pane when its bucket keeps versions, each
//! restorable (copied over the key, becoming the current one) and deletable for good, as the policies allow.

use super::browse::Details;
use super::rights;
use super::root::{Confirmed, Root};
use crate::state::format::{base_name, when};
use gpui::{Context, SharedString, Window};
use rs3manager_core::s3::Version;
use rs3manager_core::s3::versions::versioned;

/// What the details pane shows of an object's versions.
pub enum VersionsState {
    /// Nothing: the bucket doesn't keep versions, or the policies don't allow listing them.
    Hidden,
    /// Being listed.
    Loading,
    /// Listed, the current one first.
    Ready(Vec<Version>),
    /// Couldn't be listed, and why.
    Failed(SharedString),
}

impl Root {
    /// Lists object `key`'s versions into the details, when the policies may allow it.
    pub(super) fn load_versions(&mut self, key: &str, cx: &mut Context<Self>) {
        let Some(bucket) = self.browser.bucket().map(str::to_owned) else { return };
        if !self.may(rights::LIST_VERSIONS, None) {
            return;
        }
        if let Some(d) = self.details.as_mut().filter(|d| d.key == key) {
            d.versions = VersionsState::Loading;
        }
        let (for_key, k) = (key.to_owned(), key.to_owned());
        self.run(
            cx,
            move |s| s.versions(&bucket, &k),
            move |this, result, _| {
                let Some(d) = this.details.as_mut().filter(|d| d.key == for_key) else { return };
                d.versions = match result {
                    Ok(versions) if versioned(&versions) => VersionsState::Ready(versions),
                    Ok(_) => VersionsState::Hidden,
                    Err(e) => VersionsState::Failed(e.to_string().into()),
                };
            },
        );
    }

    /// Reads the selected object's details again, and the folder (its size or date may have changed, or it may be
    /// gone): after a version was restored or deleted.
    fn reload_details(&mut self, key: &str, cx: &mut Context<Self>) {
        if self.details.as_ref().is_some_and(|d| d.key == key) {
            self.details = Some(Details::new(key.to_owned()));
            self.load_details(key.to_owned(), cx);
        }
        self.reload_folder(cx);
    }

    /// Makes version `id` of the selected object the current one.
    pub fn restore_version(&mut self, id: String, cx: &mut Context<Self>) {
        let (Some(bucket), Some(details)) = (self.browser.bucket().map(str::to_owned), &self.details) else { return };
        let key = details.key.clone();
        let k = key.clone();
        self.run(
            cx,
            move |s| s.restore_version(&bucket, &k, &id),
            move |this, result, cx| {
                match result {
                    Ok(_) => this.info(format!("Restored an earlier version of {}", base_name(&key)), cx),
                    Err(e) => this.error(format!("Couldn't restore the version of {key}: {e}"), cx),
                }
                this.reload_details(&key, cx);
            },
        );
    }

    /// Asks before deleting `version` of the selected object.
    pub fn ask_delete_version(&mut self, version: &Version, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(bucket), Some(details)) = (self.browser.bucket().map(str::to_owned), &self.details) else { return };
        let key = details.key.clone();
        let name = base_name(&key).to_owned();
        let (title, body) = if version.delete_marker && version.latest {
            (format!("Bring {name} back?"), "Deleting this delete marker makes the version before it current again.")
        } else if version.delete_marker {
            (format!("Delete this delete marker of {name}?"), "It isn't the current version: nothing else changes.")
        } else if version.latest {
            (
                format!("Delete the current version of {name}?"),
                "The version before it becomes current; with none, the object is gone. This can't be undone.",
            )
        } else {
            (
                format!("Delete the version of {name} from {}?", version.modified.map_or_else(String::new, when)),
                "This version is deleted for good. This can't be undone.",
            )
        };
        let action = Confirmed::DeleteVersion { bucket, key, id: version.id.clone() };
        self.confirm(title, body, action, window, cx);
    }

    /// Deletes version `id` of object `key` in `bucket`.
    pub(super) fn delete_version(&mut self, bucket: String, key: String, id: String, cx: &mut Context<Self>) {
        let k = key.clone();
        self.run(
            cx,
            move |s| s.delete_version(&bucket, &k, &id),
            move |this, result, cx| {
                match result {
                    Ok(()) => this.info(format!("Deleted a version of {}", base_name(&key)), cx),
                    Err(e) => this.error(format!("Couldn't delete the version of {key}: {e}"), cx),
                }
                this.reload_details(&key, cx);
            },
        );
    }

    /// Whether the selected object's versions may be restored, and deleted.
    pub fn version_rights(&self, key: &str) -> (bool, bool) {
        (self.may_on_key(rights::RESTORE_VERSION, key), self.may_on_key(rights::DELETE_VERSION, key))
    }
}
