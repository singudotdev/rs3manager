//! Share links: a presigned download URL for the selected object, for someone without the profile's keys, as the
//! `MinIO` console's "Share" gave.

use super::root::{Modal, Root};
use gpui::{ClipboardItem, Context, SharedString, Window};
use std::time::{Duration, SystemTime};

/// How long a link works. Seven days is `SigV4`'s longest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expiry {
    /// One hour.
    Hour,
    /// One day.
    Day,
    /// Seven days.
    Week,
}

impl Expiry {
    /// Every choice, shortest first.
    pub const ALL: [Self; 3] = [Self::Hour, Self::Day, Self::Week];

    /// Its button's label.
    pub fn label(self) -> &'static str {
        match self {
            Self::Hour => "1 hour",
            Self::Day => "1 day",
            Self::Week => "7 days",
        }
    }

    /// How long that is.
    pub fn duration(self) -> Duration {
        let hours = match self {
            Self::Hour => 1,
            Self::Day => 24,
            Self::Week => 7 * 24,
        };
        Duration::from_secs(hours * 3600)
    }
}

/// The share dialog of one object.
pub struct ShareDialog {
    /// The object's bucket.
    pub bucket: String,
    /// The object.
    pub key: String,
    /// How long the link works.
    pub expiry: Expiry,
    /// The link for that expiry, once signed, and when it stops working.
    pub link: Option<(SharedString, SystemTime)>,
    /// Why it couldn't be signed.
    pub error: Option<SharedString>,
}

impl Root {
    /// Opens the share dialog for the selected object, with a one-day link signed straight away.
    pub fn open_share(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(details), Some(bucket)) = (&self.details, self.browser.bucket()) else { return };
        self.modal = Some(Modal::Share(ShareDialog {
            bucket: bucket.to_owned(),
            key: details.key.clone(),
            expiry: Expiry::Day,
            link: None,
            error: None,
        }));
        window.focus(&self.focus, cx);
        self.sign_link(cx);
    }

    /// Picks how long the link works, and signs a new one.
    pub fn set_share_expiry(&mut self, expiry: Expiry, cx: &mut Context<Self>) {
        let Some(Modal::Share(dialog)) = &mut self.modal else { return };
        if dialog.expiry != expiry {
            dialog.expiry = expiry;
            self.sign_link(cx);
        }
    }

    /// Signs the dialog's link for its expiry. An answer arriving after the expiry was changed again, or the dialog
    /// closed, is dropped, so the link shown is always the one for the choice shown.
    fn sign_link(&mut self, cx: &mut Context<Self>) {
        let Some(Modal::Share(dialog)) = &mut self.modal else { return };
        dialog.link = None;
        dialog.error = None;
        let (bucket, key, expiry) = (dialog.bucket.clone(), dialog.key.clone(), dialog.expiry);
        let until = SystemTime::now() + expiry.duration();
        let wanted = key.clone();
        self.run(
            cx,
            |s| s.share(&bucket, &key, expiry.duration()),
            move |this, result, _| {
                let Some(Modal::Share(dialog)) = &mut this.modal else { return };
                if dialog.expiry != expiry || dialog.key != wanted {
                    return;
                }
                match result {
                    Ok(link) => dialog.link = Some((link.into(), until)),
                    Err(e) => dialog.error = Some(e.to_string().into()),
                }
            },
        );
        cx.notify();
    }

    /// Copies the dialog's link, once signed.
    pub fn copy_share_link(&mut self, cx: &mut Context<Self>) {
        let Some(Modal::Share(ShareDialog { link: Some((link, _)), .. })) = &self.modal else { return };
        cx.write_to_clipboard(ClipboardItem::new_string(link.to_string()));
        self.info("Copied the link", cx);
    }
}
