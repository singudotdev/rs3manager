//! The window's state: the profiles and the session as one of them, the buckets, the open folder, the details shown,
//! the transfers, the dialog open and the last message. Every operation runs on the core's engine and comes back
//! through [`Root::run`], which drops answers meant for a session that's been replaced since.

use super::browse::Details;
use super::input::{Changed, TextInput};
use super::profiles::{ProfileMenu, ProfilesDialog};
use super::share::ShareDialog;
use crate::state::browser::{Browser, Row};
use crate::state::transfers::{Job, Transfers};
use gpui::{
    AppContext as _, AsyncApp, Context, Entity, FocusHandle, Focusable as _, SharedString, Subscription,
    UniformListScrollHandle, WeakEntity, Window,
};
use rs3manager_core::profiles::{Files, Profile, selected_by_env};
use rs3manager_core::s3::{Access, Bucket, Engine, Pending, S3Error, Session};
use std::time::Duration;

/// How long a message that isn't an error stays.
const NOTICE_FOR: Duration = Duration::from_secs(5);

/// A dialog over the window.
pub enum Modal {
    /// Asks before doing something that can't be undone.
    Confirm {
        /// The question.
        title: SharedString,
        /// What will happen.
        body: SharedString,
        /// What confirming does.
        action: Confirmed,
    },
    /// Asks for a name.
    Prompt {
        /// What's asked.
        title: SharedString,
        /// The field.
        input: Entity<TextInput>,
        /// What submitting does.
        action: Prompted,
        /// Why the last name typed was refused.
        error: Option<SharedString>,
        /// Whether the gateway is being asked.
        busy: bool,
        /// What may be typed, shown under the field.
        hint: Option<SharedString>,
    },
    /// The dropdown switching profiles.
    ProfileMenu(ProfileMenu),
    /// The AWS CLI profiles: pick, add, edit, remove.
    Profiles(ProfilesDialog),
    /// A download link to an object.
    Share(ShareDialog),
    /// Uploads that would replace objects already there: replace them, skip them, or upload nothing.
    Replace {
        /// Every upload asked for.
        jobs: Vec<Job>,
        /// The keys among them that already exist.
        existing: Vec<String>,
    },
}

/// What a confirmation does.
pub enum Confirmed {
    /// Deletes these rows of `bucket`.
    DeleteRows {
        /// The bucket.
        bucket: String,
        /// The rows.
        rows: Vec<Row>,
    },
    /// Deletes bucket `name`, emptying it first when `force`.
    DeleteBucket {
        /// The bucket.
        name: String,
        /// Whether to empty it first.
        force: bool,
    },
    /// Removes profile `name` from the AWS CLI's files.
    RemoveProfile(String),
}

/// What submitting a name does.
pub enum Prompted {
    /// Creates a bucket of that name.
    CreateBucket,
    /// Creates a folder of that name in the open folder.
    CreateFolder,
    /// Renames or moves `row` to that key.
    Rename(Row),
}

/// The message at the bottom of the window.
pub struct Notice {
    /// What it says.
    pub text: SharedString,
    /// Whether it's an error, which stays until dismissed.
    pub error: bool,
    /// Its number, so a timer only dismisses the one it was set for.
    id: u64,
}

/// The window's root view.
#[expect(clippy::struct_excessive_bools, reason = "independent flags of separate areas, not a state machine")]
pub struct Root {
    /// Runs every S3 operation.
    pub engine: Engine,
    /// The AWS CLI's files.
    pub files: Files,
    /// Their profiles, as last read.
    pub profiles: Vec<Profile>,
    /// The profile chosen, connected or not.
    pub profile: Option<String>,
    /// The session as that profile, once connected.
    pub session: Option<Session>,
    /// Bumped by every connection, so answers for an earlier session are dropped.
    session_generation: u64,
    /// The account's buckets the profile may see.
    pub buckets: Vec<Bucket>,
    /// Whether the last listing of the buckets failed, so the sidebar doesn't say the account has none.
    pub buckets_failed: bool,
    /// What the profile may do, as its policies say.
    pub access: Access,
    /// Whether the buckets are being listed.
    pub buckets_loading: bool,
    /// The folder shown.
    pub browser: Browser,
    /// The selected object's details.
    pub details: Option<Details>,
    /// Uploads and downloads.
    pub transfers: Transfers,
    /// Whether the transfers list is expanded.
    pub transfers_open: bool,
    /// The dialog open.
    pub modal: Option<Modal>,
    /// The last message.
    pub notice: Option<Notice>,
    /// The number of the last message.
    notice_id: u64,
    /// The filter over the folder's rows.
    pub filter: Entity<TextInput>,
    /// The window's focus when no field has it, where the browser's keys go.
    pub focus: FocusHandle,
    /// The rows' scroll.
    pub list_scroll: UniformListScrollHandle,
    /// Whether the transfers' progress is being redrawn on a timer.
    pub ticking: bool,
    /// Whether the next frame gives the keys back to the browser: set where there's no window to focus with.
    pub refocus: bool,
    /// Keeps the filter's subscription alive.
    _subscriptions: Vec<Subscription>,
}

impl Root {
    /// The window's state, connecting to the profile `AWS_PROFILE` names (else `default`, else the first one).
    pub fn new(engine: Engine, files: Files, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let filter = cx.new(|cx| TextInput::new("Filter this folder", cx));
        let subscription = cx.subscribe(&filter, |this: &mut Self, input, _: &Changed, cx| {
            let text = input.read(cx).text().to_owned();
            this.browser.set_filter(&text);
            this.sync_details(cx);
            cx.notify();
        });
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let mut root = Self {
            engine,
            files,
            profiles: Vec::new(),
            profile: None,
            session: None,
            session_generation: 0,
            buckets: Vec::new(),
            buckets_failed: false,
            buckets_loading: false,
            access: Access::Unknown("not connected".to_owned()),
            browser: Browser::default(),
            details: None,
            transfers: Transfers::default(),
            transfers_open: false,
            modal: None,
            notice: None,
            notice_id: 0,
            filter,
            focus,
            list_scroll: UniformListScrollHandle::new(),
            ticking: false,
            refocus: false,
            _subscriptions: vec![subscription],
        };
        root.reload_profiles(cx);
        let wanted = selected_by_env();
        let first = root.profiles.iter().find(|p| p.name == wanted).or(root.profiles.first()).map(|p| p.name.clone());
        match first {
            Some(name) => root.connect(name, cx),
            None => root.open_profiles(cx),
        }
        root
    }

    /// Reads the profiles again from the AWS CLI's files.
    pub fn reload_profiles(&mut self, cx: &mut Context<Self>) {
        match self.files.load() {
            Ok(profiles) => self.profiles = profiles,
            Err(e) => self.error(format!("Couldn't read the AWS CLI profiles: {e}"), cx),
        }
    }

    /// Switches to profile `name`: forgets what the last one showed and connects.
    pub fn connect(&mut self, name: String, cx: &mut Context<Self>) {
        self.session = None;
        self.session_generation += 1;
        self.access = Access::Unknown("not connected".to_owned());
        self.buckets.clear();
        self.browser.open(None, String::new());
        self.details = None;
        self.profile = Some(name.clone());
        let generation = self.session_generation;
        let pending = self.engine.connect(&self.files, &name);
        self.buckets_loading = true;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = pending.await;
            let _ = this.update(cx, |this, cx| {
                if this.session_generation != generation {
                    return;
                }
                match result {
                    Ok(session) => {
                        this.session = Some(session);
                        this.load_access(cx);
                    }
                    Err(e) => {
                        this.buckets_loading = false;
                        this.error(format!("Couldn't use profile {name}: {e}"), cx);
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Forgets the session and what it showed: no profile left to use.
    pub fn disconnect(&mut self, cx: &mut Context<Self>) {
        self.session = None;
        self.session_generation += 1;
        self.profile = None;
        self.access = Access::Unknown("not connected".to_owned());
        self.buckets.clear();
        self.buckets_loading = false;
        self.browser.open(None, String::new());
        self.details = None;
        cx.notify();
    }

    /// Runs `start` on the current session and hands its result to `done`, unless the session was replaced
    /// meanwhile. Nothing runs without a session.
    pub fn run<T: 'static>(
        &mut self,
        cx: &mut Context<Self>,
        start: impl FnOnce(&Session) -> Pending<T>,
        done: impl FnOnce(&mut Self, Result<T, S3Error>, &mut Context<Self>) + 'static,
    ) {
        let Some(session) = &self.session else { return };
        let pending = start(session);
        let generation = self.session_generation;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = pending.await;
            let _ = this.update(cx, |this, cx| {
                if this.session_generation == generation {
                    done(this, result, cx);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Shows `text` as an error, until dismissed or replaced.
    pub fn error(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        let text = text.into();
        tracing::warn!(target: "rs3manager::ui", message = %text, "error shown");
        self.notice_id += 1;
        self.notice = Some(Notice { text, error: true, id: self.notice_id });
        cx.notify();
    }

    /// Shows `text` for a few seconds.
    pub fn info(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.notice_id += 1;
        let id = self.notice_id;
        self.notice = Some(Notice { text: text.into(), error: false, id });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor().timer(NOTICE_FOR).await;
            let _ = this.update(cx, |this, cx| {
                if this.notice.as_ref().is_some_and(|n| n.id == id) {
                    this.notice = None;
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }

    /// Closes the dialog and gives the keys back to the browser.
    pub fn close_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.modal = None;
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Opens a dialog asking for a name, the field starting with `text` and `selected` of it selected.
    pub fn prompt(
        &mut self,
        title: impl Into<SharedString>,
        text: &str,
        selected: std::ops::Range<usize>,
        action: Prompted,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = cx.new(|cx| {
            let mut input = TextInput::new("", cx).with_text(text.to_owned());
            input.select(selected, cx);
            input
        });
        let field = input.read(cx).focus_handle(cx);
        window.focus(&field, cx);
        self.modal = Some(Modal::Prompt { title: title.into(), input, action, error: None, busy: false, hint: None });
        cx.notify();
    }

    /// Opens a dialog asking to confirm `action`.
    pub fn confirm(
        &mut self,
        title: impl Into<SharedString>,
        body: impl Into<SharedString>,
        action: Confirmed,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.modal = Some(Modal::Confirm { title: title.into(), body: body.into(), action });
        window.focus(&self.focus, cx);
        cx.notify();
    }
}
