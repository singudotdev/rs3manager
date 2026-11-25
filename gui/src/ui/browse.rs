//! Buckets, folders and objects: listing them, opening them, selecting rows, the selected object's details and its
//! editable headers, previews, and the dialogs that create, rename and delete.

use super::input::TextInput;
use super::rights;
use super::root::{Confirmed, Modal, Prompted, Root};
use crate::state::browser::Row;
use crate::state::format::{base_name, key_in, name_in, parent};
use gpui::{AppContext as _, ClipboardItem, Context, Entity, Image, ImageFormat, SharedString, Window};
use rs3manager_core::s3::names::url_path;
use rs3manager_core::s3::{Access, Headers, ObjectInfo, Preview, S3Error};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The most of a text object shown.
const TEXT_PREVIEW: u64 = 256 * 1024;

/// The largest image shown; larger ones are only downloaded.
const IMAGE_PREVIEW: u64 = 16 * 1024 * 1024;

/// What the details panel shows of an object's contents.
pub enum PreviewState {
    /// Being fetched.
    Loading,
    /// Text, cut at [`TEXT_PREVIEW`] when `truncated`.
    Text(SharedString, bool),
    /// An image.
    Image(Arc<Image>),
    /// Nothing to show: binary, empty or too large, with why.
    None(SharedString),
}

/// The selected object's details.
pub struct Details {
    /// Its key.
    pub key: String,
    /// What `HeadObject` said, once it answered.
    pub info: Option<ObjectInfo>,
    /// Its contents.
    pub preview: PreviewState,
    /// Its headers, editable.
    pub form: Option<HeadersForm>,
    /// Whether the headers are being saved.
    pub saving: bool,
}

/// An object's headers as fields.
pub struct HeadersForm {
    /// `Content-Type`.
    pub content_type: Entity<TextInput>,
    /// `Cache-Control`.
    pub cache_control: Entity<TextInput>,
    /// `Content-Disposition`.
    pub content_disposition: Entity<TextInput>,
    /// `Content-Encoding`.
    pub content_encoding: Entity<TextInput>,
    /// `Content-Language`.
    pub content_language: Entity<TextInput>,
    /// User metadata, a name and value field per entry.
    pub user: Vec<(Entity<TextInput>, Entity<TextInput>)>,
}

impl HeadersForm {
    /// Fields holding `headers`.
    fn new(headers: &Headers, cx: &mut Context<Root>) -> Self {
        let field = |value: &Option<String>, hint: &str, cx: &mut Context<Root>| {
            let value = value.clone().unwrap_or_default();
            let hint = hint.to_owned();
            cx.new(|cx| TextInput::new(hint, cx).with_text(value))
        };
        Self {
            content_type: field(&headers.content_type, "application/octet-stream", cx),
            cache_control: field(&headers.cache_control, "e.g. max-age=3600", cx),
            content_disposition: field(&headers.content_disposition, "e.g. attachment", cx),
            content_encoding: field(&headers.content_encoding, "e.g. gzip", cx),
            content_language: field(&headers.content_language, "e.g. en", cx),
            user: headers.user.iter().map(|(k, v)| user_row(k, v, cx)).collect(),
        }
    }

    /// The headers the fields hold: empty ones unset, names lowercase as S3 stores them.
    fn headers(&self, cx: &Context<Root>) -> Headers {
        let value = |input: &Entity<TextInput>| Some(input.read(cx).text().trim().to_owned()).filter(|v| !v.is_empty());
        let user: BTreeMap<String, String> = self
            .user
            .iter()
            .filter_map(|(k, v)| {
                let name = k.read(cx).text().trim().to_lowercase();
                (!name.is_empty()).then(|| (name, v.read(cx).text().trim().to_owned()))
            })
            .collect();
        Headers {
            content_type: value(&self.content_type),
            cache_control: value(&self.cache_control),
            content_disposition: value(&self.content_disposition),
            content_encoding: value(&self.content_encoding),
            content_language: value(&self.content_language),
            user,
        }
    }
}

/// A user metadata entry's two fields.
fn user_row(name: &str, value: &str, cx: &mut Context<Root>) -> (Entity<TextInput>, Entity<TextInput>) {
    let (name, value) = (name.to_owned(), value.to_owned());
    (cx.new(|cx| TextInput::new("name", cx).with_text(name)), cx.new(|cx| TextInput::new("value", cx).with_text(value)))
}

/// The image format to show an object as, from its type or name; `None` when it isn't one GPUI draws.
fn image_format(content_type: Option<&str>, key: &str) -> Option<ImageFormat> {
    let by_type = match content_type.map(|t| t.split(';').next().unwrap_or(t).trim()) {
        Some("image/png") => Some(ImageFormat::Png),
        Some("image/jpeg") => Some(ImageFormat::Jpeg),
        Some("image/gif") => Some(ImageFormat::Gif),
        Some("image/webp") => Some(ImageFormat::Webp),
        Some("image/bmp") => Some(ImageFormat::Bmp),
        Some("image/svg+xml") => Some(ImageFormat::Svg),
        _ => None,
    };
    by_type.or_else(|| match key.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).as_deref() {
        Some("png") => Some(ImageFormat::Png),
        Some("jpg" | "jpeg") => Some(ImageFormat::Jpeg),
        Some("gif") => Some(ImageFormat::Gif),
        Some("webp") => Some(ImageFormat::Webp),
        Some("bmp") => Some(ImageFormat::Bmp),
        Some("svg") => Some(ImageFormat::Svg),
        _ => None,
    })
}

/// What a text preview of `preview` shows, or `None` when it isn't text. A cut preview may end inside a character,
/// which is dropped; any other invalid UTF-8 or a NUL byte means binary.
fn as_text(preview: &Preview) -> Option<String> {
    let bytes = &preview.bytes;
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(e) if preview.truncated && e.error_len().is_none() => {
            std::str::from_utf8(&bytes[..e.valid_up_to()]).ok()?
        }
        Err(_) => return None,
    };
    (!text.contains('\0')).then(|| text.to_owned())
}

impl Root {
    /// Lists the account's buckets the profile may see: radosgw lists every bucket of the account, so those its
    /// policies keep it out of are left out (judged by the policies when known, else by trying each).
    pub fn load_buckets(&mut self, cx: &mut Context<Self>) {
        self.buckets_loading = true;
        let check = matches!(self.access, Access::Unknown(_));
        self.run(
            cx,
            move |s| s.list_buckets(check),
            |this, result, cx| {
                this.buckets_loading = false;
                match result {
                    Ok(mut buckets) => {
                        buckets.retain(|b| b.listable && this.access.may("s3:ListBucket", &b.name, None));
                        this.buckets = buckets;
                    }
                    Err(e) => this.error(format!("Couldn't list the buckets: {e}"), cx),
                }
            },
        );
    }

    /// Opens bucket `name` at its top.
    pub fn open_bucket(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.open_folder(Some(name.to_owned()), String::new(), window, cx);
    }

    /// Opens folder `prefix` of `bucket`, clearing the filter.
    pub fn open_folder(&mut self, bucket: Option<String>, prefix: String, window: &mut Window, cx: &mut Context<Self>) {
        let generation = self.browser.open(bucket, prefix);
        self.filter.update(cx, |input, cx| input.set_text("", cx));
        self.details = None;
        self.list_scroll.scroll_to_item(0, gpui::ScrollStrategy::Top);
        window.focus(&self.focus, cx);
        self.list(generation, None, cx);
    }

    /// Whether the open bucket (if any) may be listed by the profile, as last checked.
    pub fn open_bucket_listable(&self) -> bool {
        let open = self.browser.bucket();
        self.buckets.iter().find(|b| Some(b.name.as_str()) == open).is_none_or(|b| b.listable)
    }

    /// Lists a page of the open folder for `generation`, from `token`. A bucket the profile can't list isn't asked:
    /// the list says so in place of rows.
    fn list(&mut self, generation: u64, token: Option<String>, cx: &mut Context<Self>) {
        let Some(bucket) = self.browser.bucket().map(str::to_owned) else { return };
        if !self.open_bucket_listable() {
            self.browser.failed(generation);
            cx.notify();
            return;
        }
        let prefix = self.browser.prefix().to_owned();
        let first = token.is_none();
        let (b, p) = (bucket.clone(), prefix.clone());
        self.run(
            cx,
            move |s| s.list(&b, &p, token),
            move |this, result, cx| match result {
                Ok(page) => {
                    if this.browser.loaded(generation, first, page) {
                        this.sync_details(cx);
                    }
                }
                // The bucket's top denied: its access changed since the buckets were listed.
                Err(S3Error::AccessDenied(_)) if prefix.is_empty() => {
                    this.browser.failed(generation);
                    if let Some(b) = this.buckets.iter_mut().find(|b| b.name == bucket) {
                        b.listable = false;
                    }
                }
                Err(e) => {
                    this.browser.failed(generation);
                    this.error(format!("Couldn't list {bucket}/{prefix}: {e}"), cx);
                }
            },
        );
        cx.notify();
    }

    /// Loads the open folder's next page.
    pub fn load_more(&mut self, cx: &mut Context<Self>) {
        if let Some((generation, token)) = self.browser.more() {
            self.list(generation, Some(token), cx);
        }
    }

    /// Lists the open folder again.
    pub fn reload_folder(&mut self, cx: &mut Context<Self>) {
        let generation = self.browser.reload();
        self.list(generation, None, cx);
    }

    /// Reads the profile's policies again (they may have changed), then the buckets and the open folder.
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        self.load_access(cx);
        self.reload_folder(cx);
    }

    /// Opens the folder above the open one.
    pub fn go_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(bucket) = self.browser.bucket().map(str::to_owned)
            && !self.browser.prefix().is_empty()
        {
            let up = parent(self.browser.prefix()).to_owned();
            self.open_folder(Some(bucket), up, window, cx);
        }
    }

    /// A click on row `index`: a plain click opens a folder or selects an object; with ctrl/cmd or shift it selects.
    pub fn click_row(&mut self, index: usize, toggle: bool, range: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(row) = self.browser.row(index).cloned() else { return };
        window.focus(&self.focus, cx);
        match row {
            Row::Folder(prefix) if !toggle && !range => {
                let bucket = self.browser.bucket().map(str::to_owned);
                self.open_folder(bucket, prefix, window, cx);
            }
            row => {
                self.browser.click(row.key(), toggle, range);
                self.sync_details(cx);
            }
        }
        cx.notify();
    }

    /// Toggles row `index` in the selection, as its checkbox does.
    pub fn toggle_row(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(key) = self.browser.row(index).map(|r| r.key().to_owned()) {
            self.browser.click(&key, true, false);
            self.sync_details(cx);
            cx.notify();
        }
    }

    /// Shows the details of the single selected object, fetching them when it changed; none otherwise.
    pub fn sync_details(&mut self, cx: &mut Context<Self>) {
        let Some(object) = self.browser.single_object().cloned() else {
            self.details = None;
            return;
        };
        if self.details.as_ref().is_some_and(|d| d.key == object.key) {
            return;
        }
        self.details = Some(Details {
            key: object.key.clone(),
            info: None,
            preview: PreviewState::Loading,
            form: None,
            saving: false,
        });
        self.load_details(object.key, cx);
    }

    /// Fetches `key`'s details, then its preview.
    fn load_details(&mut self, key: String, cx: &mut Context<Self>) {
        let Some(bucket) = self.browser.bucket().map(str::to_owned) else { return };
        // HeadObject needs the same right as reading it: known denied, it isn't asked.
        if !self.may_on_key(rights::READ, &key) {
            if let Some(details) = self.details.as_mut() {
                details.preview = PreviewState::None("This profile's policies don't allow reading this object.".into());
            }
            return;
        }
        let for_key = key.clone();
        let b = bucket.clone();
        self.run(
            cx,
            move |s| s.head(&b, &key),
            move |this, result, cx| {
                let Some(details) = this.details.as_mut().filter(|d| d.key == for_key) else { return };
                match result {
                    Ok(info) => {
                        details.form = Some(HeadersForm::new(&info.headers, cx));
                        let image = image_format(info.headers.content_type.as_deref(), &info.key);
                        let size = info.size;
                        details.info = Some(info);
                        this.load_preview(&bucket, &for_key, size, image, cx);
                    }
                    Err(e) => {
                        details.preview = PreviewState::None("".into());
                        this.error(format!("Couldn't read {for_key}: {e}"), cx);
                    }
                }
            },
        );
    }

    /// Fetches what the details panel shows of object `key` (`size` bytes), as an image when `image` says it is one.
    fn load_preview(&mut self, bucket: &str, key: &str, size: u64, image: Option<ImageFormat>, cx: &mut Context<Self>) {
        let set = |this: &mut Self, key: &str, preview| {
            if let Some(d) = this.details.as_mut().filter(|d| d.key == key) {
                d.preview = preview;
            }
        };
        if size == 0 {
            set(self, key, PreviewState::None("Empty object".into()));
            return;
        }
        if image.is_some() && size > IMAGE_PREVIEW {
            set(self, key, PreviewState::None("Image too large to preview".into()));
            return;
        }
        let limit = if image.is_some() { IMAGE_PREVIEW } else { TEXT_PREVIEW };
        let for_key = key.to_owned();
        self.run(
            cx,
            |s| s.preview(bucket, key, limit),
            move |this, result, _cx| {
                let preview = match (result, image) {
                    (Ok(p), Some(format)) => PreviewState::Image(Arc::new(Image::from_bytes(format, p.bytes))),
                    (Ok(p), None) => match as_text(&p) {
                        Some(text) => PreviewState::Text(text.into(), p.truncated),
                        None => PreviewState::None("Binary object: no preview".into()),
                    },
                    (Err(e), _) => PreviewState::None(format!("Couldn't preview: {e}").into()),
                };
                set(this, &for_key, preview);
            },
        );
    }

    /// Adds an empty user metadata entry to the form.
    pub fn add_metadata_row(&mut self, cx: &mut Context<Self>) {
        let row = user_row("", "", cx);
        if let Some(form) = self.details.as_mut().and_then(|d| d.form.as_mut()) {
            form.user.push(row);
            cx.notify();
        }
    }

    /// Removes user metadata entry `index` from the form.
    pub fn remove_metadata_row(&mut self, index: usize, cx: &mut Context<Self>) {
        if let Some(form) = self.details.as_mut().and_then(|d| d.form.as_mut())
            && index < form.user.len()
        {
            form.user.remove(index);
            cx.notify();
        }
    }

    /// Puts the form's headers back as the object has them.
    pub fn reset_headers(&mut self, cx: &mut Context<Self>) {
        let Some(details) = self.details.as_mut() else { return };
        if let Some(info) = &details.info {
            details.form = Some(HeadersForm::new(&info.headers, cx));
            cx.notify();
        }
    }

    /// Saves the form's headers on the object.
    pub fn save_headers(&mut self, cx: &mut Context<Self>) {
        let (Some(bucket), Some(details)) = (self.browser.bucket().map(str::to_owned), self.details.as_mut()) else {
            return;
        };
        let Some(form) = &details.form else { return };
        let headers = form.headers(cx);
        if let Err(e) = headers.check() {
            self.error(e.to_string(), cx);
            return;
        }
        details.saving = true;
        let key = details.key.clone();
        let k = key.clone();
        self.run(
            cx,
            move |s| s.set_headers(&bucket, &k, headers),
            move |this, result, cx| {
                if let Some(d) = this.details.as_mut().filter(|d| d.key == key) {
                    d.saving = false;
                }
                match result {
                    Ok(info) => {
                        if let Some(d) = this.details.as_mut().filter(|d| d.key == key) {
                            d.form = Some(HeadersForm::new(&info.headers, cx));
                            d.info = Some(info);
                        }
                        this.info(format!("Saved the headers of {}", base_name(&key)), cx);
                    }
                    Err(e) => this.error(format!("Couldn't save the headers of {key}: {e}"), cx),
                }
            },
        );
        cx.notify();
    }

    /// Asks for a new bucket's name.
    pub fn ask_create_bucket(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.may_create_buckets() {
            return;
        }
        self.prompt("New bucket", "", 0..0, Prompted::CreateBucket, window, cx);
        if let (Some(Modal::Prompt { hint, .. }), Some(patterns)) = (&mut self.modal, self.access.bucket_patterns()) {
            *hint = Some(format!("Allowed names: {}", patterns.join(", ")).into());
        }
    }

    /// Asks for a new folder's name in the open folder.
    pub fn ask_create_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.browser.bucket().is_some() {
            self.prompt("New folder", "", 0..0, Prompted::CreateFolder, window, cx);
        }
    }

    /// Asks for the selected row's new name: its key relative to the open folder, the name without its extension
    /// selected.
    pub fn ask_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let selection = self.browser.selection();
        let [row] = selection.as_slice() else { return };
        if !self.may_on_selection(rights::RENAME) {
            return;
        }
        let row = (*row).clone();
        let shown = name_in(row.key(), self.browser.prefix()).to_owned();
        let end = match &row {
            Row::Folder(_) => shown.len() - 1,
            Row::Object(_) => shown.rfind('.').filter(|&i| i > 0).unwrap_or(shown.len()),
        };
        self.prompt("Rename or move", &shown, 0..end, Prompted::Rename(row), window, cx);
    }

    /// Asks before deleting the selected rows.
    pub fn ask_delete_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(bucket) = self.browser.bucket().map(str::to_owned) else { return };
        if !self.may_on_selection(rights::DELETE) {
            return;
        }
        let rows: Vec<Row> = self.browser.selection().into_iter().cloned().collect();
        let (folders, objects) = rows.iter().fold((0, 0), |(f, o), r| match r {
            Row::Folder(_) => (f + 1, o),
            Row::Object(_) => (f, o + 1),
        });
        let what = match (folders, objects) {
            (0, 0) => return,
            (0, 1) | (1, 0) => format!("“{}”", base_name(rows[0].key())),
            (0, n) => format!("{n} objects"),
            (n, 0) => format!("{n} folders"),
            (f, o) => format!("{f} folders and {o} objects"),
        };
        let body = if folders > 0 {
            "Every object inside the folders is deleted too. This can't be undone."
        } else {
            "This can't be undone."
        };
        self.confirm(format!("Delete {what}?"), body, Confirmed::DeleteRows { bucket, rows }, window, cx);
    }

    /// Asks before deleting bucket `name`.
    pub fn ask_delete_bucket(&mut self, name: &str, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        if !self.may_delete_bucket(name, force) {
            return;
        }
        let body = if force {
            "Every object, version and unfinished upload in it is deleted first. This can't be undone."
        } else {
            "Only an empty bucket can be deleted this way."
        };
        let action = Confirmed::DeleteBucket { name: name.to_owned(), force };
        self.confirm(format!("Delete bucket “{name}”?"), body, action, window, cx);
    }

    /// Does what the confirmation open asked.
    pub fn confirmed(&mut self, action: Confirmed, window: &mut Window, cx: &mut Context<Self>) {
        self.close_modal(window, cx);
        match action {
            Confirmed::DeleteRows { bucket, rows } => self.delete_rows(&bucket, &rows, cx),
            Confirmed::DeleteBucket { name, force } => self.delete_bucket(name, force, window, cx),
            Confirmed::RemoveProfile(name) => self.remove_profile(&name, window, cx),
        }
    }

    /// Deletes `rows` of `bucket`: the objects in batches, each folder with everything under it.
    fn delete_rows(&mut self, bucket: &str, rows: &[Row], cx: &mut Context<Self>) {
        let keys: Vec<String> =
            rows.iter().filter(|r| matches!(r, Row::Object(_))).map(|r| r.key().to_owned()).collect();
        if !keys.is_empty() {
            self.run(cx, |s| s.delete(bucket, keys), Self::deleted);
        }
        for folder in rows.iter().filter(|r| matches!(r, Row::Folder(_))) {
            self.run(cx, |s| s.delete_folder(bucket, folder.key()), Self::deleted);
        }
        self.browser.clear_selection();
        self.details = None;
    }

    /// Reports a delete and lists the folder again.
    fn deleted(&mut self, result: Result<rs3manager_core::s3::DeleteReport, S3Error>, cx: &mut Context<Self>) {
        match result {
            Ok(report) if report.failed.is_empty() => {
                let n = report.deleted;
                self.info(format!("Deleted {n} object{}", if n == 1 { "" } else { "s" }), cx);
            }
            Ok(report) => {
                let (key, why) = &report.failed[0];
                let more = report.failed.len() - 1;
                let rest = if more > 0 { format!(" (and {more} more)") } else { String::new() };
                self.error(format!("Deleted {}, but not {key}: {why}{rest}", report.deleted), cx);
            }
            Err(e) => self.error(format!("Couldn't delete: {e}"), cx),
        }
        self.reload_folder(cx);
    }

    /// Deletes bucket `name`.
    fn delete_bucket(&mut self, name: String, force: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.browser.bucket() == Some(name.as_str()) {
            self.open_folder(None, String::new(), window, cx);
        }
        let n = name.clone();
        self.run(
            cx,
            move |s| s.delete_bucket(&n, force),
            move |this, result, cx| {
                match result {
                    Ok(()) => this.info(format!("Deleted bucket {name}"), cx),
                    Err(S3Error::BucketNotEmpty) => {
                        this.error(
                            format!("Bucket {name} isn't empty: use “Empty and delete” to delete what's in it too"),
                            cx,
                        );
                    }
                    Err(e) => this.error(format!("Couldn't delete bucket {name}: {e}"), cx),
                }
                this.load_buckets(cx);
            },
        );
    }

    /// Does what the name dialog open asked, keeping it open with the reason when the name is refused.
    pub fn submit_prompt(&mut self, cx: &mut Context<Self>) {
        let Some(Modal::Prompt { input, action, busy: false, .. }) = &self.modal else { return };
        let name = input.read(cx).text().trim().to_owned();
        if name.is_empty() {
            return;
        }
        let bucket = self.browser.bucket().map(str::to_owned);
        let prefix = self.browser.prefix().to_owned();
        match action {
            Prompted::CreateBucket => {
                // A name the policies surely refuse is refused here, with the names they allow, without a round trip.
                if !self.access.may("s3:CreateBucket", &name, None) {
                    let allowed = self.access.bucket_patterns().unwrap_or_default().join(", ");
                    self.prompt_failed(format!("This profile's policies only allow creating: {allowed}"), cx);
                    return;
                }
                let n = name.clone();
                self.run(
                    cx,
                    move |s| s.create_bucket(&n),
                    move |this, result, cx| match result {
                        Ok(()) => {
                            this.prompt_done(cx);
                            this.info(format!("Created bucket {name}"), cx);
                            this.load_buckets(cx);
                        }
                        // Accounts commonly limit the names a user may create, e.g. to project-name-env.
                        Err(S3Error::AccessDenied(_)) => this.prompt_failed(
                            format!(
                                "This profile's policies don't allow creating {name}. They may allow only some names \
                                 (often a pattern such as project-name-env), or no new buckets at all."
                            ),
                            cx,
                        ),
                        Err(e) => this.prompt_failed(e.to_string(), cx),
                    },
                );
            }
            Prompted::CreateFolder => {
                let Some(bucket) = bucket else { return };
                let folder = format!("{}/", key_in(&prefix, &name).trim_end_matches('/'));
                self.run(
                    cx,
                    |s| s.create_folder(&bucket, &folder),
                    |this, result, cx| match result {
                        Ok(()) => {
                            this.prompt_done(cx);
                            this.reload_folder(cx);
                        }
                        Err(e) => this.prompt_failed(e.to_string(), cx),
                    },
                );
            }
            Prompted::Rename(row) => {
                let Some(bucket) = bucket else { return };
                let target = key_in(&prefix, &name);
                match row.clone() {
                    Row::Object(object) => {
                        let from = object.key;
                        self.run(
                            cx,
                            |s| s.rename(&bucket, &from, &target),
                            |this, result, cx| match result {
                                Ok(()) => {
                                    this.prompt_done(cx);
                                    this.reload_folder(cx);
                                }
                                Err(e) => this.prompt_failed(e.to_string(), cx),
                            },
                        );
                    }
                    Row::Folder(from) => {
                        let target = format!("{}/", target.trim_end_matches('/'));
                        self.run(
                            cx,
                            |s| s.rename_folder(&bucket, &from, &target),
                            |this, result, cx| match result {
                                Ok(n) => {
                                    this.prompt_done(cx);
                                    this.info(format!("Moved {n} object{}", if n == 1 { "" } else { "s" }), cx);
                                    this.reload_folder(cx);
                                }
                                Err(e) => this.prompt_failed(e.to_string(), cx),
                            },
                        );
                    }
                }
            }
        }
        if let Some(Modal::Prompt { busy, error, .. }) = &mut self.modal {
            *busy = true;
            *error = None;
        }
        cx.notify();
    }

    /// Closes the name dialog once the gateway took the name.
    fn prompt_done(&mut self, cx: &mut Context<Self>) {
        if matches!(self.modal, Some(Modal::Prompt { .. })) {
            self.modal = None;
            self.refocus = true;
            cx.notify();
        }
    }

    /// Keeps the name dialog open with why the gateway refused the name, to fix it.
    fn prompt_failed(&mut self, why: String, cx: &mut Context<Self>) {
        if let Some(Modal::Prompt { busy, error, .. }) = &mut self.modal {
            *busy = false;
            *error = Some(why.into());
        } else {
            self.error(why, cx);
        }
    }

    /// Copies the selected object's key.
    pub fn copy_key(&mut self, cx: &mut Context<Self>) {
        if let Some(d) = &self.details {
            cx.write_to_clipboard(ClipboardItem::new_string(d.key.clone()));
            self.info("Copied the key", cx);
        }
    }

    /// Copies the selected object's path-style URL on the gateway, as an app would fetch it.
    pub fn copy_url(&mut self, cx: &mut Context<Self>) {
        let (Some(d), Some(bucket), Some(endpoint)) =
            (&self.details, self.browser.bucket(), self.session.as_ref().and_then(|s| s.endpoint()))
        else {
            return;
        };
        let url = format!("{}/{bucket}/{}", endpoint.trim_end_matches('/'), url_path(&d.key));
        cx.write_to_clipboard(ClipboardItem::new_string(url));
        self.info("Copied the object's URL", cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previews_tell_text_from_binary() {
        let text = |bytes: &[u8], truncated| as_text(&Preview { bytes: bytes.to_vec(), truncated });
        assert_eq!(text(b"hello", false).as_deref(), Some("hello"));
        // Cut inside "ñ" (0xC3 0xB1): the half character is dropped.
        assert_eq!(text(b"a\xC3", true).as_deref(), Some("a"));
        assert_eq!(text(b"a\xC3", false), None);
        assert_eq!(text(b"\x89PNG\0\0", false), None);
    }

    #[test]
    fn images_are_known_by_type_or_name() {
        assert_eq!(image_format(Some("image/png"), "x"), Some(ImageFormat::Png));
        assert_eq!(image_format(Some("application/octet-stream"), "a/b.JPG"), Some(ImageFormat::Jpeg));
        assert_eq!(image_format(None, "notes.txt"), None);
    }
}
