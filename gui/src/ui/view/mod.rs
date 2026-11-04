//! Drawing the window: the layout and its key actions here, each area in its own module.
//!
//! - [`panes`]: the top bar, the buckets, and the folder's path and toolbar.
//! - [`list`]: the folder's rows.
//! - [`details`]: the selected object's details, preview and headers.
//! - [`overlays`]: transfers, the message bar and dialogs.

mod details;
mod list;
mod overlays;
mod panes;

use super::profiles::ProfilesDialog;
use super::root::{Modal, Root};
use super::theme::{self, SANS};
use super::{Cancel, DeleteSelected, FocusFilter, GoUp, Refresh, RenameSelected, SelectAllRows, Submit};
use gpui::{Context, ExternalPaths, Focusable as _, Window, div, prelude::*};

impl Root {
    /// Handles [`Submit`]: the dialog's main button.
    fn on_submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        match &self.modal {
            Some(Modal::Prompt { .. }) => self.submit_prompt(cx),
            Some(Modal::Confirm { .. }) => {
                if let Some(Modal::Confirm { action, .. }) = self.modal.take() {
                    self.confirmed(action, window, cx);
                }
            }
            Some(Modal::Profiles(ProfilesDialog { form: Some(_) })) => self.save_profile(cx),
            Some(Modal::Share(_)) => self.copy_share_link(cx),
            Some(Modal::Profiles(ProfilesDialog { form: None })) | None => {}
        }
    }

    /// Handles [`Cancel`]: closes the dialog (a profile form back to the list), else clears the selection.
    fn on_cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<Self>) {
        match &self.modal {
            Some(Modal::Profiles(ProfilesDialog { form: Some(_) })) => self.cancel_profile_form(window, cx),
            Some(_) => self.close_modal(window, cx),
            None => {
                self.browser.clear_selection();
                self.sync_details(cx);
                window.focus(&self.focus, cx);
                cx.notify();
            }
        }
    }

    /// Runs a browser shortcut, unless a dialog is open.
    fn browser_key(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>),
    ) {
        if self.modal.is_none() {
            f(self, window, cx);
        }
    }
}

impl Render for Root {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if std::mem::take(&mut self.refocus) {
            window.focus(&self.focus, cx);
        }
        let theme = theme::of(window);
        let body = div()
            .flex()
            .flex_1()
            .min_h_0()
            .child(self.sidebar(&theme, cx))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .child(self.toolbar(&theme, window, cx))
                    .child(self.rows(&theme, cx)),
            )
            .children(self.details_pane(&theme, cx));
        div()
            .id("root")
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.bg)
            .text_color(theme.text)
            .font_family(SANS)
            .text_sm()
            .key_context(if self.modal.is_some() { "Dialog" } else { "Browser" })
            .track_focus(&self.focus.clone())
            .on_action(cx.listener(Self::on_submit))
            .on_action(cx.listener(Self::on_cancel))
            .on_action(cx.listener(|this, _: &Refresh, w, cx| this.browser_key(w, cx, |this, _, cx| this.refresh(cx))))
            .on_action(
                cx.listener(|this, _: &DeleteSelected, w, cx| this.browser_key(w, cx, Self::ask_delete_selected)),
            )
            .on_action(cx.listener(|this, _: &RenameSelected, w, cx| this.browser_key(w, cx, Self::ask_rename)))
            .on_action(cx.listener(|this, _: &GoUp, w, cx| this.browser_key(w, cx, Self::go_up)))
            .on_action(cx.listener(|this, _: &SelectAllRows, w, cx| {
                this.browser_key(w, cx, |this, _, cx| {
                    this.browser.toggle_all();
                    this.sync_details(cx);
                    cx.notify();
                });
            }))
            .on_action(cx.listener(|this, _: &FocusFilter, w, cx| {
                this.browser_key(w, cx, |this, window, cx| {
                    let field = this.filter.focus_handle(cx);
                    window.focus(&field, cx);
                });
            }))
            .on_drop(cx.listener(|this, paths: &ExternalPaths, _, cx| this.upload_paths(paths.paths().to_vec(), cx)))
            .child(self.topbar(&theme, cx))
            .child(body)
            .children(self.transfers_pane(&theme, cx))
            .children(self.notice_bar(&theme, cx))
            .children(self.dialog(&theme, cx))
    }
}
