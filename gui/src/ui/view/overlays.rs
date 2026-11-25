//! What sits over or under the panes: the transfers list, the message bar, and the dialog open.

use super::super::profiles::{ProfileForm, ProfilesDialog};
use super::super::root::{Modal, Root};
use super::super::share::{Expiry, ShareDialog};
use super::super::theme::{MONO, Theme};
use super::super::widgets::{Kind, button, label};
use crate::state::format::{base_name, size, when};
use crate::state::transfers::{Job, Status, Transfer};
use gpui::{Context, Div, Stateful, div, prelude::*, px, relative};
use rs3manager_core::profiles::Credentials;
use std::fmt::Write as _;

impl Root {
    /// The transfers: a summary line, and the list when expanded.
    pub(super) fn transfers_pane(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Option<Div> {
        let items = self.transfers.items();
        if items.is_empty() {
            return None;
        }
        let (done, total, unfinished) = self.transfers.totals();
        let failed = items.iter().filter(|t| matches!(t.status, Status::Failed(_))).count();
        let mut summary = if unfinished > 0 {
            format!(
                "{unfinished} transfer{} · {} of {}",
                if unfinished == 1 { "" } else { "s" },
                size(done),
                size(total)
            )
        } else {
            "Transfers finished".to_owned()
        };
        if failed > 0 {
            let _ = write!(summary, " · {failed} failed");
        }
        let open = self.transfers_open;
        let header = div()
            .flex()
            .items_center()
            .gap_2()
            .px_4()
            .h(px(36.))
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(summary))
            .child(div().flex_1())
            .child(button("t-toggle", if open { "Hide" } else { "Show" }, Kind::Quiet, false, theme).on_click(
                cx.listener(|this, _, _, cx| {
                    this.transfers_open = !this.transfers_open;
                    cx.notify();
                }),
            ))
            .child(button("t-clear", "Clear finished", Kind::Quiet, false, theme).on_click(cx.listener(
                |this, _, _, cx| {
                    this.transfers.clear_finished();
                    cx.notify();
                },
            )))
            .child(button("t-cancel", "Cancel all", Kind::Quiet, unfinished == 0, theme).on_click(cx.listener(
                |this, _, _, cx| {
                    this.transfers.cancel_all();
                    cx.notify();
                },
            )));
        let rows = open.then(|| {
            div()
                .id("transfers")
                .flex()
                .flex_col()
                .max_h(px(180.))
                .overflow_y_scroll()
                .children(items.iter().map(|t| transfer_row(t, theme, cx)))
        });
        Some(
            div()
                .flex()
                .flex_col()
                .bg(theme.panel)
                .border_t_1()
                .border_color(theme.border)
                .child(header)
                .children(rows),
        )
    }

    /// The last message, with a button to dismiss it.
    pub(super) fn notice_bar(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Option<Div> {
        let notice = self.notice.as_ref()?;
        let color = if notice.error { theme.danger } else { theme.text };
        Some(
            div()
                .flex()
                .items_center()
                .gap_3()
                .px_4()
                .py_2()
                .bg(theme.panel)
                .border_t_1()
                .border_color(theme.border)
                .child(div().flex_1().min_w_0().text_color(color).child(notice.text.clone()))
                .child(button("notice-close", "Dismiss", Kind::Quiet, false, theme).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.notice = None;
                        cx.notify();
                    },
                ))),
        )
    }

    /// The dialog open, over a dimmed window.
    pub(super) fn dialog(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Option<Div> {
        let content = match self.modal.as_ref()? {
            Modal::Confirm { title, body, .. } => card(theme, 440.)
                .child(heading(title.clone()))
                .child(div().text_color(theme.muted).child(body.clone()))
                .child(footer().child(cancel(theme, cx)).child(
                    button("confirm", "Delete", Kind::Danger, false, theme).on_click(cx.listener(
                        |this, _, window, cx| {
                            if let Some(Modal::Confirm { action, .. }) = this.modal.take() {
                                this.confirmed(action, window, cx);
                            }
                        },
                    )),
                )),
            Modal::Prompt { title, input, error, busy, hint, .. } => card(theme, 440.)
                .child(heading(title.clone()))
                .child(input.clone())
                .children(hint.clone().map(|h| div().text_xs().text_color(theme.muted).child(h)))
                .children(error.clone().map(|e| div().text_xs().text_color(theme.danger).child(e)))
                .child(
                    footer().child(cancel(theme, cx)).child(
                        button("ok", if *busy { "Working…" } else { "OK" }, Kind::Primary, *busy, theme)
                            .on_click(cx.listener(|this, _, _, cx| this.submit_prompt(cx))),
                    ),
                ),
            Modal::Profiles(ProfilesDialog { form: Some(form) }) => Self::profile_form(form, theme, cx),
            Modal::Profiles(ProfilesDialog { form: None }) => self.profile_list(theme, cx),
            Modal::Share(dialog) => Self::share_dialog(dialog, theme, cx),
            // Drawn under the profile button, without dimming the window.
            Modal::ProfileMenu(_) => return None,
            Modal::Replace { jobs, existing } => Self::replace_dialog(jobs.len(), existing, theme, cx),
        };
        Some(div().absolute().inset_0().flex().items_center().justify_center().bg(theme.scrim).occlude().child(content))
    }

    /// The question before an upload of `total` files of which `existing` are already there. Both answers name what
    /// they do to those files, the ones listed ("Overwrite 3", "Skip 3"), so neither reads as a choice about the
    /// rest. Skipping stays even when every file is there and it uploads nothing, since uploading a folder again is
    /// the common case and a dialog without the safe answer reads as "overwrite or give up".
    fn replace_dialog(total: usize, existing: &[String], theme: &Theme, cx: &mut Context<Self>) -> Div {
        const SHOWN: usize = 6;
        let count = existing.len();
        let plural = |n: usize| if n == 1 { "" } else { "s" };
        let title = match (count, total) {
            (1, 1) => format!("{} is already there", base_name(&existing[0])),
            (c, t) if c == t => format!("All {t} files are already there"),
            (c, t) => format!("{c} of {t} files are already there"),
        };
        let names = existing.iter().take(SHOWN).map(|k| div().truncate().child(k.clone()));
        let more = (count > SHOWN).then(|| format!("and {} more", count - SHOWN));
        card(theme, 480.)
            .child(heading(title.into()))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .font_family(MONO)
                    .text_xs()
                    .children(names)
                    .children(more.map(|m| div().text_color(theme.muted).child(m))),
            )
            .child(div().text_color(theme.muted).child(format!(
                "Overwriting replaces what's in the bucket with your file{}; it can't be undone.",
                plural(count)
            )))
            .child(
                footer()
                    .child(cancel(theme, cx))
                    .child(
                        button("overwrite", format!("Overwrite {count}"), Kind::Danger, false, theme)
                            .on_click(cx.listener(|this, _, window, cx| this.replace_chosen(true, window, cx))),
                    )
                    .child(
                        button("skip", format!("Skip {count}"), Kind::Primary, false, theme)
                            .on_click(cx.listener(|this, _, window, cx| this.replace_chosen(false, window, cx))),
                    ),
            )
    }

    /// The profile dropdown, while open: the search field, the matching profiles (the current one marked, the
    /// highlighted one Enter picks), and the way to the full dialog. A click outside closes it.
    pub(super) fn profile_menu(&self, theme: &Theme, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let Some(Modal::ProfileMenu(menu)) = &self.modal else { return None };
        let current = self.profile.as_deref();
        let matches = self.menu_matches(cx);
        let rows = matches.iter().enumerate().map(|(i, p)| {
            let name = p.name.clone();
            let selector = format!("menu profile {name}");
            div()
                .id(("menu-profile", i))
                .debug_selector(move || selector)
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .py_1p5()
                .rounded_md()
                .cursor_pointer()
                .when(i == menu.highlighted, |d| d.bg(theme.selected))
                .when(i != menu.highlighted, |d| d.hover(|s| s.bg(theme.hover)))
                .child(div().w(px(12.)).text_color(theme.accent).child(if current == Some(name.as_str()) {
                    "✓"
                } else {
                    ""
                }))
                .child(
                    div().flex().flex_col().min_w_0().child(div().truncate().child(name.clone())).child(
                        div()
                            .text_xs()
                            .text_color(theme.muted)
                            .truncate()
                            .child(p.endpoint_url.clone().unwrap_or_else(|| "AWS (no endpoint_url)".to_owned())),
                    ),
                )
                .on_click(cx.listener(move |this, _, window, cx| this.pick_profile(name.clone(), window, cx)))
        });
        let empty = matches
            .is_empty()
            .then(|| div().px_3().py_2().text_xs().text_color(theme.muted).child("No profile matches"));
        Some(
            div()
                .id("profile-menu")
                .occlude()
                .flex()
                .flex_col()
                .gap_1()
                .w(px(360.))
                .p_2()
                .rounded_lg()
                .border_1()
                .border_color(theme.border)
                .bg(theme.raised)
                .shadow_lg()
                .on_mouse_down_out(cx.listener(|this, _, window, cx| {
                    if matches!(this.modal, Some(Modal::ProfileMenu(_))) {
                        this.close_modal(window, cx);
                    }
                }))
                .child(menu.query.clone())
                .child(
                    div()
                        .id("profile-matches")
                        .flex()
                        .flex_col()
                        .max_h(px(320.))
                        .overflow_y_scroll()
                        .children(rows)
                        .children(empty),
                )
                .child(
                    div().border_t_1().border_color(theme.border).pt_1().child(
                        button("manage-profiles", "Manage profiles…", Kind::Quiet, false, theme)
                            .on_click(cx.listener(|this, _, _, cx| this.open_profiles(cx))),
                    ),
                ),
        )
    }

    /// The profiles to pick from.
    fn profile_list(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let current = self.profile.clone();
        let rows = self.profiles.iter().enumerate().map(|(i, p)| {
            let (pick, edit, remove) = (p.name.clone(), p.name.clone(), p.name.clone());
            let status = match p.credentials {
                Credentials::Keys => p.access_key_id.clone().unwrap_or_default(),
                Credentials::External => "credentials from SSO or a process".to_owned(),
                Credentials::Missing => "no credentials".to_owned(),
            };
            let is_current = current.as_deref() == Some(p.name.as_str());
            div()
                .id(("profile", i))
                .flex()
                .items_center()
                .gap_3()
                .px_3()
                .py_2()
                .rounded_md()
                .when(is_current, |d| d.bg(theme.selected))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_w_0()
                        .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(p.name.clone()))
                        .child(
                            div()
                                .text_xs()
                                .text_color(theme.muted)
                                .truncate()
                                .child(p.endpoint_url.clone().unwrap_or_else(|| "AWS (no endpoint_url)".to_owned())),
                        )
                        .child(div().text_xs().font_family(MONO).text_color(theme.muted).truncate().child(status)),
                )
                .child(
                    button(("use", i), "Use", Kind::Primary, false, theme)
                        .on_click(cx.listener(move |this, _, window, cx| this.pick_profile(pick.clone(), window, cx))),
                )
                .child(
                    button(("edit", i), "Edit", Kind::Plain, false, theme)
                        .on_click(cx.listener(move |this, _, window, cx| this.edit_profile(Some(&edit), window, cx))),
                )
                .child(
                    button(("remove", i), "Remove", Kind::Quiet, false, theme)
                        .on_click(cx.listener(move |this, _, window, cx| this.ask_remove_profile(&remove, window, cx))),
                )
        });
        let files = format!("{} · {}", self.files.config.display(), self.files.credentials.display());
        card(theme, 560.)
            .child(heading("Profiles".into()))
            .child(div().text_xs().text_color(theme.muted).child(files))
            .child(div().id("profiles").flex().flex_col().gap_1().max_h(px(360.)).overflow_y_scroll().children(rows))
            .when(self.profiles.is_empty(), |d| d.child(div().text_color(theme.muted).child("No profiles yet.")))
            .child(
                footer()
                    .child(
                        button("add-profile", "Add profile", Kind::Plain, false, theme)
                            .on_click(cx.listener(|this, _, window, cx| this.edit_profile(None, window, cx))),
                    )
                    .child(
                        button("close", "Close", Kind::Primary, false, theme)
                            .on_click(cx.listener(|this, _, window, cx| this.close_modal(window, cx))),
                    ),
            )
    }

    /// The share dialog: how long the link works, the link, and what holding it allows.
    fn share_dialog(dialog: &ShareDialog, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let choices = Expiry::ALL.into_iter().map(|expiry| {
            let kind = if expiry == dialog.expiry { Kind::Primary } else { Kind::Plain };
            button(expiry.label(), expiry.label(), kind, false, theme)
                .on_click(cx.listener(move |this, _, _, cx| this.set_share_expiry(expiry, cx)))
        });
        let name = base_name(&dialog.key).to_owned();
        let (link, note) = match (&dialog.link, &dialog.error) {
            (_, Some(e)) => (div().text_xs().text_color(theme.danger).child(e.clone()), None),
            (Some((link, until)), None) => (
                div().text_xs().font_family(MONO).child(link.clone()),
                Some(format!(
                    "Anyone with this link can download {name} until {}. It can't be revoked, short of changing \
                     the profile's keys.",
                    when(*until)
                )),
            ),
            (None, None) => (div().text_xs().text_color(theme.muted).child("Signing…"), None),
        };
        card(theme, 520.)
            .child(heading(format!("Share {name}").into()))
            .child(label("Link works for", theme))
            .child(div().flex().gap_1().children(choices))
            .child(
                div()
                    .max_h(px(160.))
                    .overflow_hidden()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.bg)
                    .p_2()
                    .debug_selector(|| "share link".to_owned())
                    .child(link),
            )
            .children(note.map(|n| div().text_xs().text_color(theme.muted).child(n)))
            .child(
                footer()
                    .child(
                        button("share-close", "Close", Kind::Plain, false, theme)
                            .on_click(cx.listener(|this, _, window, cx| this.close_modal(window, cx))),
                    )
                    .child(
                        button("share-copy", "Copy link", Kind::Primary, dialog.link.is_none(), theme)
                            .on_click(cx.listener(|this, _, _, cx| this.copy_share_link(cx))),
                    ),
            )
    }

    /// The form adding or editing a profile.
    fn profile_form(form: &ProfileForm, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let field = |name: &'static str, input| div().flex().flex_col().gap_1().child(label(name, theme)).child(input);
        let title = match &form.original {
            Some(name) => format!("Edit profile {name}"),
            None => "New profile".to_owned(),
        };
        card(theme, 480.)
            .child(heading(title.into()))
            .child(field("Name", form.name.clone()))
            .child(field("Endpoint URL", form.endpoint.clone()))
            .child(field("Region", form.region.clone()))
            .child(field("Access key ID", form.access_key.clone()))
            .child(field("Secret access key", form.secret.clone()))
            .when(form.external, |d| {
                d.child(div().text_xs().text_color(theme.muted).child(
                    "Its credentials come from SSO or a process; keys typed here are added to the credentials file.",
                ))
            })
            .children(form.error.clone().map(|e| div().text_xs().text_color(theme.danger).child(e)))
            .child(
                footer()
                    .child(
                        button("form-cancel", "Back", Kind::Plain, false, theme)
                            .on_click(cx.listener(|this, _, window, cx| this.cancel_profile_form(window, cx))),
                    )
                    .child(
                        button(
                            "form-save",
                            if form.saving { "Saving…" } else { "Save" },
                            Kind::Primary,
                            form.saving,
                            theme,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.save_profile(cx))),
                    ),
            )
    }
}

/// One transfer: which way, the object, a progress bar, where it is, and a button to cancel it while it can be.
fn transfer_row(t: &Transfer, theme: &Theme, cx: &mut Context<Root>) -> Div {
    let (moved, of) = t.progress.get();
    #[expect(clippy::cast_precision_loss, reason = "a bar's width")]
    let fraction = if t.status == Status::Done {
        1.
    } else if of == 0 {
        0.
    } else {
        moved as f32 / of as f32
    };
    let (arrow, key) = match &t.job {
        Job::Upload { key, .. } => ("↑", key),
        Job::Download { key, .. } => ("↓", key),
    };
    let (state, color) = match &t.status {
        Status::Queued => ("Queued".to_owned(), theme.muted),
        Status::Running => (format!("{} / {}", size(moved), size(of)), theme.muted),
        Status::Done => ("Done".to_owned(), theme.muted),
        Status::Failed(why) => (why.clone(), theme.danger),
        Status::Cancelled => ("Cancelled".to_owned(), theme.muted),
    };
    let id = t.id;
    let active = matches!(t.status, Status::Queued | Status::Running);
    div()
        .flex()
        .items_center()
        .gap_3()
        .px_4()
        .h(px(28.))
        .child(div().w(px(14.)).text_color(theme.muted).child(arrow))
        .child(div().w(px(260.)).flex_none().truncate().child(base_name(key).to_owned()))
        .child(
            div()
                .w(px(160.))
                .flex_none()
                .h(px(6.))
                .rounded_full()
                .bg(theme.hover)
                .child(div().h_full().rounded_full().bg(theme.accent).w(relative(fraction))),
        )
        .child(div().flex_1().min_w_0().truncate().text_xs().text_color(color).child(state))
        .children(active.then(|| {
            button(("t-stop", id), "Cancel", Kind::Quiet, false, theme).on_click(cx.listener(move |this, _, _, cx| {
                this.transfers.cancel(id);
                cx.notify();
            }))
        }))
}

/// A dialog's box, `width` wide.
fn card(theme: &Theme, width: f32) -> Div {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .w(px(width))
        .p_5()
        .rounded_lg()
        .border_1()
        .border_color(theme.border)
        .bg(theme.raised)
        .shadow_lg()
}

/// A dialog's title.
fn heading(text: gpui::SharedString) -> Div {
    div().text_base().font_weight(gpui::FontWeight::SEMIBOLD).child(text)
}

/// A dialog's buttons, on the right.
fn footer() -> Div {
    div().flex().justify_end().gap_2().pt_2()
}

/// The button closing a dialog without doing anything.
fn cancel(theme: &Theme, cx: &mut Context<Root>) -> gpui::Stateful<Div> {
    button("cancel", "Cancel", Kind::Plain, false, theme)
        .on_click(cx.listener(|this, _, window, cx| this.close_modal(window, cx)))
}
