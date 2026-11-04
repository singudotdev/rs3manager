//! Switching profiles from a dropdown with a search field, and the profiles dialog: the AWS CLI's profiles, and a form
//! to add or edit one. Writing the files runs on a background thread; the core keeps everything else in them as it
//! was.

use super::input::{Changed, TextInput};
use super::root::{Confirmed, Modal, Root};
use gpui::{
    AppContext as _, AsyncApp, Context, Entity, Focusable as _, SharedString, Subscription, WeakEntity, Window,
};
use r3manager_core::profiles::{Credentials, Profile, ProfileEdit};

/// The dropdown under the profile button: a search field and the profiles it matches.
pub struct ProfileMenu {
    /// What's typed.
    pub query: Entity<TextInput>,
    /// The match ↑/↓ moved to, which Enter picks.
    pub highlighted: usize,
    /// Keeps the highlight on the first match as the query changes.
    _typing: Subscription,
}

/// Whether `profile` matches `query`: every word of it, ignoring case, in its name or endpoint.
pub fn matches(profile: &Profile, query: &str) -> bool {
    let haystack = format!("{} {}", profile.name, profile.endpoint_url.as_deref().unwrap_or_default()).to_lowercase();
    query.to_lowercase().split_whitespace().all(|word| haystack.contains(word))
}

/// The profiles dialog.
pub struct ProfilesDialog {
    /// The form, while adding or editing one.
    pub form: Option<ProfileForm>,
}

/// A profile's settings as fields.
pub struct ProfileForm {
    /// The profile edited, `None` for a new one.
    pub original: Option<String>,
    /// Its name.
    pub name: Entity<TextInput>,
    /// The gateway's URL.
    pub endpoint: Entity<TextInput>,
    /// The signing region.
    pub region: Entity<TextInput>,
    /// The access key id.
    pub access_key: Entity<TextInput>,
    /// The secret, masked; left empty, the stored one stays.
    pub secret: Entity<TextInput>,
    /// Whether the profile's credentials come from elsewhere (SSO, a process), so keys typed would replace them.
    pub external: bool,
    /// Why the last save was refused.
    pub error: Option<SharedString>,
    /// Whether it's being saved.
    pub saving: bool,
}

impl Root {
    /// Opens the profile dropdown, its search field focused.
    pub fn open_profile_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reload_profiles(cx);
        let query = cx.new(|cx| TextInput::new("Search profiles", cx));
        let typing = cx.subscribe(&query, |this: &mut Self, _, _: &Changed, cx| {
            if let Some(Modal::ProfileMenu(menu)) = &mut this.modal {
                menu.highlighted = 0;
                cx.notify();
            }
        });
        window.focus(&query.read(cx).focus_handle(cx), cx);
        self.modal = Some(Modal::ProfileMenu(ProfileMenu { query, highlighted: 0, _typing: typing }));
        cx.notify();
    }

    /// The profiles the dropdown's query matches, in the files' order.
    pub fn menu_matches(&self, cx: &gpui::App) -> Vec<&Profile> {
        let Some(Modal::ProfileMenu(menu)) = &self.modal else { return Vec::new() };
        let query = menu.query.read(cx).text();
        self.profiles.iter().filter(|p| matches(p, query)).collect()
    }

    /// Moves the dropdown's highlight to the next match (or the previous), wrapping around.
    pub fn move_highlight(&mut self, forward: bool, cx: &mut Context<Self>) {
        let count = self.menu_matches(cx).len();
        if let Some(Modal::ProfileMenu(menu)) = &mut self.modal
            && count > 0
        {
            menu.highlighted =
                if forward { (menu.highlighted + 1) % count } else { (menu.highlighted + count - 1) % count };
            cx.notify();
        }
    }

    /// Switches to the dropdown's highlighted match.
    pub fn pick_highlighted(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Modal::ProfileMenu(menu)) = &self.modal else { return };
        let picked = self.menu_matches(cx).get(menu.highlighted).map(|p| p.name.clone());
        if let Some(name) = picked {
            self.pick_profile(name, window, cx);
        }
    }

    /// Opens the profiles dialog on its list.
    pub fn open_profiles(&mut self, cx: &mut Context<Self>) {
        self.reload_profiles(cx);
        self.modal = Some(Modal::Profiles(ProfilesDialog { form: None }));
        cx.notify();
    }

    /// Switches to profile `name` and closes the dialog.
    pub fn pick_profile(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.close_modal(window, cx);
        if self.profile.as_deref() != Some(name.as_str()) || self.session.is_none() {
            self.connect(name, cx);
        }
    }

    /// Opens the form for profile `name`, or for a new one.
    pub fn edit_profile(&mut self, name: Option<&str>, window: &mut Window, cx: &mut Context<Self>) {
        let profile = name.and_then(|n| self.profiles.iter().find(|p| p.name == n)).cloned();
        let text = |value: Option<String>, hint: &str, cx: &mut Context<Self>| {
            let (value, hint) = (value.unwrap_or_default(), hint.to_owned());
            cx.new(|cx| TextInput::new(hint, cx).with_text(value))
        };
        let has_keys = profile.as_ref().is_some_and(|p| p.credentials == Credentials::Keys);
        let secret_hint = if has_keys { "unchanged" } else { "secret access key" };
        let form = ProfileForm {
            original: profile.as_ref().map(|p| p.name.clone()),
            name: text(profile.as_ref().map(|p| p.name.clone()), "e.g. team-a", cx),
            endpoint: text(profile.as_ref().and_then(|p| p.endpoint_url.clone()), "https://s3.example.com", cx),
            region: text(profile.as_ref().and_then(|p| p.region.clone()), "default", cx),
            access_key: text(profile.as_ref().and_then(|p| p.access_key_id.clone()), "access key id", cx),
            secret: cx.new(|cx| TextInput::new(secret_hint, cx).masked()),
            external: profile.as_ref().is_some_and(|p| p.credentials == Credentials::External),
            error: None,
            saving: false,
        };
        let first = form.name.focus_handle(cx);
        window.focus(&first, cx);
        self.modal = Some(Modal::Profiles(ProfilesDialog { form: Some(form) }));
        cx.notify();
    }

    /// Back from the form to the list.
    pub fn cancel_profile_form(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.modal = Some(Modal::Profiles(ProfilesDialog { form: None }));
        window.focus(&self.focus, cx);
        cx.notify();
    }

    /// Saves the form, then connects to the profile saved.
    pub fn save_profile(&mut self, cx: &mut Context<Self>) {
        let Some(Modal::Profiles(ProfilesDialog { form: Some(form) })) = &mut self.modal else { return };
        if form.saving {
            return;
        }
        let read = |input: &Entity<TextInput>| input.read(cx).text().to_owned();
        let edit = ProfileEdit::new(
            &read(&form.name),
            &read(&form.endpoint),
            &read(&form.region),
            &read(&form.access_key),
            &read(&form.secret),
        );
        let edit = match edit {
            Ok(edit) => edit,
            Err(e) => {
                form.error = Some(e.to_string().into());
                cx.notify();
                return;
            }
        };
        form.saving = true;
        form.error = None;
        let original = form.original.clone();
        let files = self.files.clone();
        let saving =
            cx.background_executor().spawn(async move { files.save(original.as_deref(), &edit).map(|()| edit) });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = saving.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(edit) => {
                        this.reload_profiles(cx);
                        this.modal = None;
                        this.refocus = true;
                        this.info(format!("Saved profile {}", edit.name()), cx);
                        this.connect(edit.name().to_owned(), cx);
                    }
                    Err(e) => {
                        if let Some(Modal::Profiles(ProfilesDialog { form: Some(form) })) = &mut this.modal {
                            form.saving = false;
                            form.error = Some(e.to_string().into());
                        }
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    /// Asks before removing profile `name` from the AWS CLI's files.
    pub fn ask_remove_profile(&mut self, name: &str, window: &mut Window, cx: &mut Context<Self>) {
        let body = "Its sections are removed from ~/.aws/config and ~/.aws/credentials; the aws command loses it too. \
                    The account and its data on the gateway stay.";
        self.confirm(format!("Remove profile “{name}”?"), body, Confirmed::RemoveProfile(name.to_owned()), window, cx);
    }

    /// Removes profile `name`, switching to another when it was the current one.
    pub fn remove_profile(&mut self, name: &str, _window: &mut Window, cx: &mut Context<Self>) {
        let files = self.files.clone();
        let removed = name.to_owned();
        let removing = cx.background_executor().spawn({
            let removed = removed.clone();
            async move { files.remove(&removed) }
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let result = removing.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => {
                    this.reload_profiles(cx);
                    this.info(format!("Removed profile {removed}"), cx);
                    if this.profile.as_deref() == Some(removed.as_str()) {
                        if let Some(next) = this.profiles.first().map(|p| p.name.clone()) {
                            this.connect(next, cx);
                        } else {
                            this.disconnect(cx);
                        }
                    }
                    this.open_profiles(cx);
                }
                Err(e) => this.error(format!("Couldn't remove profile {removed}: {e}"), cx),
            });
        })
        .detach();
    }
}
