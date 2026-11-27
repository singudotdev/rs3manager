//! A bucket's settings dialog (decision #56): its tags, lifecycle rules and CORS rules, a tab each. A tab loads its
//! configuration when shown and saves it whole, as S3 does; Remove all deletes it. A tab the profile's policies don't
//! let it read isn't shown, and one they don't let it write is read-only.

use super::input::TextInput;
use super::root::{Modal, Root};
use crate::state::settings::{self as form, CorsText, LifecycleText, Tab};
use gpui::{AppContext as _, Context, Entity, SharedString, Window};
use rs3manager_core::s3::settings::{CORS_METHODS, check_cors, check_lifecycle, check_tags};
use rs3manager_core::s3::{BucketTag, CorsRule, LifecycleRule, Pending, S3Error, Session, Versioning};

/// A one-line field of the dialog.
pub type Field = Entity<TextInput>;

/// A tag's fields.
pub struct TagFields {
    /// Its key.
    pub key: Field,
    /// Its value.
    pub value: Field,
}

/// A lifecycle rule's fields.
pub struct RuleFields {
    /// The rule as read, when it holds what the fields can't: shown, not edited, and saved back as it came.
    pub kept: Option<LifecycleRule>,
    /// Its ID.
    pub id: Field,
    /// The key prefix it applies to.
    pub prefix: Field,
    /// Whether it runs.
    pub enabled: bool,
    /// Days until objects expire.
    pub expire: Field,
    /// Days until noncurrent versions are deleted.
    pub noncurrent: Field,
    /// Days until unfinished uploads are aborted.
    pub abort: Field,
}

/// A CORS rule's fields.
pub struct CorsFields {
    /// Its ID as read, kept.
    pub id: Option<String>,
    /// The origins, comma-separated.
    pub origins: Field,
    /// Which of [`CORS_METHODS`] it allows.
    pub methods: [bool; CORS_METHODS.len()],
    /// The allowed request headers, comma-separated.
    pub allowed_headers: Field,
    /// The exposed response headers, comma-separated.
    pub expose_headers: Field,
    /// The preflight's cache time, in seconds.
    pub max_age: Field,
}

/// A tab's configuration as the gateway sent it, which Reset goes back to.
#[derive(Clone)]
pub enum Loaded {
    /// The tags.
    Tags(Vec<BucketTag>),
    /// The lifecycle rules.
    Lifecycle(Vec<LifecycleRule>),
    /// The CORS rules.
    Cors(Vec<CorsRule>),
    /// Whether the bucket keeps versions.
    Versioning(Versioning),
}

impl Loaded {
    /// Whether there's nothing to remove: versioning can't be removed, only suspended.
    pub fn is_empty(&self) -> bool {
        match self {
            Self::Versioning(_) => true,
            Self::Tags(t) => t.is_empty(),
            Self::Lifecycle(r) => r.is_empty(),
            Self::Cors(r) => r.is_empty(),
        }
    }
}

/// A tab's fields.
pub enum Fields {
    /// A row per tag.
    Tags(Vec<TagFields>),
    /// A card per lifecycle rule.
    Lifecycle(Vec<RuleFields>),
    /// A card per CORS rule.
    Cors(Vec<CorsFields>),
    /// Whether versioning is to be on (else suspended).
    Versioning(bool),
}

/// What the open tab shows.
pub enum TabState {
    /// Its configuration is being read.
    Loading,
    /// It couldn't be read, and why.
    Failed(SharedString),
    /// Read, and in fields.
    Ready {
        /// As read.
        loaded: Loaded,
        /// As edited.
        fields: Fields,
    },
}

/// The settings dialog of one bucket.
pub struct SettingsDialog {
    /// The bucket.
    pub bucket: String,
    /// The tab shown.
    pub tab: Tab,
    /// What it shows.
    pub state: TabState,
    /// Whether it's being saved or removed.
    pub saving: bool,
    /// Why the last save was refused, by the checks or the gateway.
    pub error: Option<SharedString>,
    /// Whether Remove all is asking to be confirmed.
    pub confirm_remove: bool,
    /// Bumped by every load, so an answer for a tab left meanwhile is dropped.
    generation: u64,
}

/// A field holding `text`, with `hint` while empty.
fn field(text: impl Into<SharedString>, hint: &str, cx: &mut Context<Root>) -> Field {
    let (text, hint) = (text.into(), hint.to_owned());
    cx.new(|cx| TextInput::new(hint, cx).with_text(text))
}

/// The text of `field`.
fn text(field: &Field, cx: &Context<Root>) -> String {
    field.read(cx).text().to_owned()
}

/// Fields holding `tag`.
fn tag_fields((key, value): &BucketTag, cx: &mut Context<Root>) -> TagFields {
    TagFields { key: field(key.clone(), "key", cx), value: field(value.clone(), "value", cx) }
}

/// Fields holding `rule`.
fn rule_fields(rule: &LifecycleRule, cx: &mut Context<Root>) -> RuleFields {
    RuleFields {
        kept: rule.kept.is_some().then(|| rule.clone()),
        id: field(rule.id.clone(), "ID", cx),
        prefix: field(rule.prefix.clone(), "every key", cx),
        enabled: rule.enabled,
        expire: field(form::shown(rule.expire_days), "never", cx),
        noncurrent: field(form::shown(rule.noncurrent_days), "never", cx),
        abort: field(form::shown(rule.abort_days), "never", cx),
    }
}

/// Fields holding `text`.
fn cors_fields(text: CorsText, cx: &mut Context<Root>) -> CorsFields {
    CorsFields {
        id: text.id,
        origins: field(text.origins, "https://app.example.com, *", cx),
        methods: text.methods,
        allowed_headers: field(text.allowed_headers, "none, or * for any", cx),
        expose_headers: field(text.expose_headers, "none, e.g. ETag", cx),
        max_age: field(text.max_age, "the browser's default", cx),
    }
}

/// Fields holding `loaded`.
fn fields_of(loaded: &Loaded, cx: &mut Context<Root>) -> Fields {
    match loaded {
        Loaded::Tags(tags) => Fields::Tags(tags.iter().map(|t| tag_fields(t, cx)).collect()),
        Loaded::Lifecycle(rules) => Fields::Lifecycle(rules.iter().map(|r| rule_fields(r, cx)).collect()),
        Loaded::Cors(rules) => Fields::Cors(rules.iter().map(|r| cors_fields(CorsText::of(r), cx)).collect()),
        Loaded::Versioning(v) => Fields::Versioning(*v == Versioning::Enabled),
    }
}

/// What a tab's fields hold, ready to save, or why not.
enum Edited {
    /// The tags.
    Tags(Vec<BucketTag>),
    /// The lifecycle rules.
    Lifecycle(Vec<LifecycleRule>),
    /// The CORS rules.
    Cors(Vec<CorsRule>),
    /// Versioning on, or suspended.
    Versioning(bool),
}

/// What `fields` hold, checked as the core would before sending.
fn edited(fields: &Fields, cx: &Context<Root>) -> Result<Edited, String> {
    let checked = |r: Result<(), S3Error>| r.map_err(|e| e.to_string());
    match fields {
        Fields::Tags(rows) => {
            let rows: Vec<_> = rows.iter().map(|r| (text(&r.key, cx), text(&r.value, cx))).collect();
            let tags = form::tags(&rows);
            checked(check_tags(&tags))?;
            Ok(Edited::Tags(tags))
        }
        Fields::Lifecycle(cards) => {
            let rules = cards
                .iter()
                .map(|c| match &c.kept {
                    Some(rule) => Ok(rule.clone()),
                    None => LifecycleText {
                        id: text(&c.id, cx),
                        prefix: text(&c.prefix, cx),
                        enabled: c.enabled,
                        expire: text(&c.expire, cx),
                        noncurrent: text(&c.noncurrent, cx),
                        abort: text(&c.abort, cx),
                    }
                    .rule(),
                })
                .collect::<Result<Vec<_>, _>>()?;
            checked(check_lifecycle(&rules))?;
            Ok(Edited::Lifecycle(rules))
        }
        Fields::Cors(cards) => {
            let rules = cards
                .iter()
                .enumerate()
                .map(|(i, c)| {
                    CorsText {
                        id: c.id.clone(),
                        origins: text(&c.origins, cx),
                        methods: c.methods,
                        allowed_headers: text(&c.allowed_headers, cx),
                        expose_headers: text(&c.expose_headers, cx),
                        max_age: text(&c.max_age, cx),
                    }
                    .rule(i + 1)
                })
                .collect::<Result<Vec<_>, _>>()?;
            checked(check_cors(&rules))?;
            Ok(Edited::Cors(rules))
        }
        Fields::Versioning(on) => Ok(Edited::Versioning(*on)),
    }
}

impl Root {
    /// The tabs the profile may read in bucket `bucket`.
    pub fn settings_tabs(&self, bucket: &str) -> Vec<Tab> {
        Tab::ALL.into_iter().filter(|t| self.access.may(t.read_action(), bucket, None)).collect()
    }

    /// Whether the profile may change (and remove) `tab` of bucket `bucket`.
    pub fn may_edit_settings(&self, tab: Tab, bucket: &str) -> bool {
        self.access.may(tab.write_action(), bucket, None)
    }

    /// The settings dialog, when open.
    fn settings(&mut self) -> Option<&mut SettingsDialog> {
        match &mut self.modal {
            Some(Modal::Settings(dialog)) => Some(dialog),
            _ => None,
        }
    }

    /// Opens the open bucket's settings at the first tab the profile may read.
    pub fn open_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bucket) = self.browser.bucket().map(str::to_owned) else { return };
        let Some(&tab) = self.settings_tabs(&bucket).first() else { return };
        self.modal = Some(Modal::Settings(SettingsDialog {
            bucket,
            tab,
            state: TabState::Loading,
            saving: false,
            error: None,
            confirm_remove: false,
            generation: 0,
        }));
        window.focus(&self.focus, cx);
        self.load_settings(cx);
    }

    /// Shows `tab`, read again; edits not saved in the tab left are dropped.
    pub fn show_settings_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        let Some(dialog) = self.settings() else { return };
        if dialog.tab != tab {
            dialog.tab = tab;
            self.load_settings(cx);
        }
    }

    /// Reads the open tab's configuration from the gateway.
    fn load_settings(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.settings() else { return };
        dialog.generation += 1;
        dialog.state = TabState::Loading;
        dialog.error = None;
        dialog.confirm_remove = false;
        let (bucket, tab) = (dialog.bucket.clone(), dialog.tab);
        match tab {
            Tab::Tags => self.fetch_settings(|s| s.bucket_tags(&bucket), Loaded::Tags, cx),
            Tab::Lifecycle => self.fetch_settings(|s| s.lifecycle(&bucket), Loaded::Lifecycle, cx),
            Tab::Cors => self.fetch_settings(|s| s.cors(&bucket), Loaded::Cors, cx),
            Tab::Versioning => self.fetch_settings(|s| s.versioning(&bucket), Loaded::Versioning, cx),
        }
        cx.notify();
    }

    /// Runs `start` and shows what it reads, as `wrap` makes it, unless the tab was left or reloaded meanwhile.
    fn fetch_settings<T: 'static>(
        &mut self,
        start: impl FnOnce(&Session) -> Pending<T>,
        wrap: fn(T) -> Loaded,
        cx: &mut Context<Self>,
    ) {
        let Some(generation) = self.settings().map(|d| d.generation) else { return };
        self.run(cx, start, move |this, result, cx| {
            let state = match result {
                Ok(value) => {
                    let loaded = wrap(value);
                    TabState::Ready { fields: fields_of(&loaded, cx), loaded }
                }
                Err(e) => TabState::Failed(e.to_string().into()),
            };
            if let Some(dialog) = this.settings().filter(|d| d.generation == generation) {
                dialog.state = state;
            }
        });
    }

    /// Puts the tab's fields back as read.
    pub fn reset_settings(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.settings() else { return };
        let TabState::Ready { loaded, .. } = &dialog.state else { return };
        let loaded = loaded.clone();
        let fields = fields_of(&loaded, cx);
        if let Some(dialog) = self.settings() {
            dialog.state = TabState::Ready { loaded, fields };
            dialog.error = None;
        }
        cx.notify();
    }

    /// Adds an empty tag, or a new rule.
    pub fn add_settings_row(&mut self, cx: &mut Context<Self>) {
        let Some(TabState::Ready { fields, .. }) = self.settings().map(|d| &d.state) else { return };
        let taken: Vec<String> = match fields {
            Fields::Lifecycle(cards) => cards.iter().map(|c| text(&c.id, cx)).collect(),
            Fields::Tags(_) | Fields::Cors(_) | Fields::Versioning(_) => Vec::new(),
        };
        let kind = match fields {
            Fields::Tags(_) => Tab::Tags,
            Fields::Lifecycle(_) => Tab::Lifecycle,
            Fields::Cors(_) => Tab::Cors,
            Fields::Versioning(_) => return,
        };
        // Built before borrowing the dialog again: fields need `cx`.
        let row = match kind {
            Tab::Tags => Row::Tag(tag_fields(&(String::new(), String::new()), cx)),
            Tab::Lifecycle => {
                let id = form::new_rule_id(&taken);
                Row::Rule(Box::new(rule_fields(
                    &LifecycleRule { expire_days: Some(30), ..LifecycleRule::new(&id) },
                    cx,
                )))
            }
            Tab::Cors => Row::Cors(cors_fields(CorsText::new_rule(), cx)),
            Tab::Versioning => return,
        };
        let Some(TabState::Ready { fields, .. }) = self.settings().map(|d| &mut d.state) else { return };
        match (fields, row) {
            (Fields::Tags(rows), Row::Tag(r)) => rows.push(r),
            (Fields::Lifecycle(cards), Row::Rule(r)) => cards.push(*r),
            (Fields::Cors(cards), Row::Cors(r)) => cards.push(r),
            _ => {}
        }
        cx.notify();
    }

    /// Removes tag or rule `index` from the fields.
    pub fn remove_settings_row(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(TabState::Ready { fields, .. }) = self.settings().map(|d| &mut d.state) else { return };
        let len = match fields {
            Fields::Tags(rows) => rows.len(),
            Fields::Lifecycle(cards) => cards.len(),
            Fields::Cors(cards) => cards.len(),
            Fields::Versioning(_) => 0,
        };
        if index < len {
            match fields {
                Fields::Tags(rows) => drop(rows.remove(index)),
                Fields::Lifecycle(cards) => drop(cards.remove(index)),
                Fields::Cors(cards) => drop(cards.remove(index)),
                Fields::Versioning(_) => {}
            }
        }
        cx.notify();
    }

    /// Turns lifecycle rule `index` on or off.
    pub fn toggle_rule_enabled(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(TabState::Ready { fields: Fields::Lifecycle(cards), .. }) = self.settings().map(|d| &mut d.state)
            && let Some(card) = cards.get_mut(index).filter(|c| c.kept.is_none())
        {
            card.enabled = !card.enabled;
            cx.notify();
        }
    }

    /// Allows or stops allowing method `method` (an index in [`CORS_METHODS`]) in CORS rule `index`.
    pub fn toggle_cors_method(&mut self, index: usize, method: usize, cx: &mut Context<Self>) {
        if let Some(TabState::Ready { fields: Fields::Cors(cards), .. }) = self.settings().map(|d| &mut d.state)
            && let Some(on) = cards.get_mut(index).and_then(|c| c.methods.get_mut(method))
        {
            *on = !*on;
            cx.notify();
        }
    }

    /// Chooses whether versioning is to be on, to save.
    pub fn choose_versioning(&mut self, on: bool, cx: &mut Context<Self>) {
        if let Some(TabState::Ready { fields: Fields::Versioning(chosen), .. }) = self.settings().map(|d| &mut d.state)
        {
            *chosen = on;
            cx.notify();
        }
    }

    /// Saves the tab's fields, once checked; a refusal is shown in the tab.
    pub fn save_settings(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.settings() else { return };
        if dialog.saving {
            return;
        }
        let TabState::Ready { fields, loaded } = &dialog.state else { return };
        if matches!((fields, loaded), (Fields::Versioning(false), Loaded::Versioning(Versioning::Off))) {
            dialog.error = Some("Versioning is off: there's nothing to suspend.".into());
            cx.notify();
            return;
        }
        match edited(fields, cx) {
            Ok(edited) => self.write_settings(edited, false, cx),
            Err(why) => {
                if let Some(dialog) = self.settings() {
                    dialog.error = Some(why.into());
                }
                cx.notify();
            }
        }
    }

    /// Asks before Remove all, in the dialog's footer.
    pub fn ask_remove_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.settings() {
            dialog.confirm_remove = true;
            cx.notify();
        }
    }

    /// Takes Remove all back.
    pub fn keep_settings(&mut self, cx: &mut Context<Self>) {
        if let Some(dialog) = self.settings() {
            dialog.confirm_remove = false;
            cx.notify();
        }
    }

    /// Removes the tab's whole configuration from the bucket.
    pub fn remove_settings(&mut self, cx: &mut Context<Self>) {
        let Some(dialog) = self.settings() else { return };
        let edited = match dialog.tab {
            Tab::Tags => Edited::Tags(Vec::new()),
            Tab::Lifecycle => Edited::Lifecycle(Vec::new()),
            Tab::Cors => Edited::Cors(Vec::new()),
            Tab::Versioning => return,
        };
        self.write_settings(edited, true, cx);
    }

    /// Sends `edited`, then reads the tab again so it shows what the gateway keeps.
    fn write_settings(&mut self, edited: Edited, removing: bool, cx: &mut Context<Self>) {
        let Some(dialog) = self.settings() else { return };
        dialog.saving = true;
        dialog.error = None;
        dialog.confirm_remove = false;
        let (bucket, tab, generation) = (dialog.bucket.clone(), dialog.tab, dialog.generation);
        let b = bucket.clone();
        let start = move |s: &Session| match edited {
            Edited::Tags(tags) => s.set_bucket_tags(&b, tags),
            Edited::Lifecycle(rules) => s.set_lifecycle(&b, rules),
            Edited::Cors(rules) => s.set_cors(&b, rules),
            Edited::Versioning(on) => s.set_versioning(&b, on),
        };
        self.run(cx, start, move |this, result, cx| {
            let Some(dialog) = this.settings().filter(|d| d.generation == generation) else { return };
            dialog.saving = false;
            match result {
                Ok(()) => {
                    let done = if removing { "Removed" } else { "Saved" };
                    this.info(format!("{done} the {} of {bucket}", tab.noun()), cx);
                    this.load_settings(cx);
                }
                Err(e) => dialog.error = Some(e.to_string().into()),
            }
        });
        cx.notify();
    }
}

/// A row being added, built before the dialog is borrowed to take it.
enum Row {
    /// A tag.
    Tag(TagFields),
    /// A lifecycle rule, boxed: it has the most fields.
    Rule(Box<RuleFields>),
    /// A CORS rule.
    Cors(CorsFields),
}
