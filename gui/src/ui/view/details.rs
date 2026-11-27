//! The selected object's panel: what it is, what can be done with it, its preview and its headers.

use super::super::browse::{Details, HeadersForm, PreviewState};
use super::super::rights;
use super::super::root::Root;
use super::super::theme::{MONO, Theme};
use super::super::versions::VersionsState;
use super::super::widgets::{Kind, button, label};
use crate::state::format::{base_name, size, when};
use gpui::{Context, Div, ObjectFit, Stateful, StyledImage as _, Window, div, img, prelude::*, px};

/// The panel's width, and in the compact layout.
const WIDTH: (f32, f32) = (340., 300.);

impl Root {
    /// The panel, when one object is selected.
    pub(super) fn details_pane(
        &mut self,
        theme: &Theme,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let details = self.details.as_ref()?;
        let facts = details.info.as_ref().map(|info| {
            let mut rows = vec![("Size", format!("{} ({} bytes)", size(info.size), info.size))];
            rows.extend(info.modified.map(|m| ("Modified", when(m))));
            rows.extend(info.etag.clone().map(|e| ("ETag", e)));
            rows.extend(info.version_id.clone().map(|v| ("Version", v)));
            rows.extend(info.storage_class.clone().map(|c| ("Storage class", c)));
            div().flex().flex_col().gap_1().children(rows.into_iter().map(|(k, v)| {
                div()
                    .flex()
                    .gap_2()
                    .child(div().w(px(96.)).flex_none().text_xs().text_color(theme.muted).child(k))
                    .child(div().text_xs().font_family(MONO).min_w_0().child(v))
            }))
        });
        let key = details.key.as_str();
        let (read, rename, delete) = (
            self.may_on_key(rights::READ, key),
            self.may_on_key(rights::RENAME, key),
            self.may_on_key(rights::DELETE, key),
        );
        let actions = div()
            .flex()
            .flex_wrap()
            .gap_1()
            .when(read, |d| {
                d.child(
                    button("d-download", "Download", Kind::Primary, false, theme)
                        .on_click(cx.listener(|this, _, _, cx| this.download_selected(cx))),
                )
            })
            .when(rename, |d| {
                d.child(
                    button("d-rename", "Rename", Kind::Plain, false, theme)
                        .on_click(cx.listener(|this, _, window, cx| this.ask_rename(window, cx))),
                )
            })
            .when(delete, |d| {
                d.child(
                    button("d-delete", "Delete", Kind::Danger, false, theme)
                        .on_click(cx.listener(|this, _, window, cx| this.ask_delete_selected(window, cx))),
                )
            })
            .when(read, |d| {
                d.child(
                    button("d-share", "Share link", Kind::Plain, false, theme)
                        .on_click(cx.listener(|this, _, window, cx| this.open_share(window, cx))),
                )
            })
            .child(
                button("d-key", "Copy key", Kind::Quiet, false, theme)
                    .on_click(cx.listener(|this, _, _, cx| this.copy_key(cx))),
            )
            .child(
                button(
                    "d-url",
                    "Copy URL",
                    Kind::Quiet,
                    self.session.as_ref().and_then(|s| s.endpoint()).is_none(),
                    theme,
                )
                .on_click(cx.listener(|this, _, _, cx| this.copy_url(cx))),
            );
        Some(
            div()
                .id("details")
                .flex()
                .flex_col()
                .gap_4()
                .w(px(if super::compact(window) { WIDTH.1 } else { WIDTH.0 }))
                .flex_none()
                .p_4()
                .bg(theme.panel)
                .border_l_1()
                .border_color(theme.border)
                .overflow_y_scroll()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .text_base()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(base_name(&details.key).to_owned()),
                        )
                        .child(div().text_xs().font_family(MONO).text_color(theme.muted).child(details.key.clone())),
                )
                .child(actions)
                .children(facts)
                .child(section("Preview", theme).child(preview(details, theme)))
                .children(self.versions_section(details, theme, cx))
                .children(details.form.as_ref().map(|form| {
                    let editable = self.may_on_key(rights::EDIT_HEADERS, &details.key);
                    Self::headers_form(form, details, editable, theme, cx)
                })),
        )
    }

    /// The headers form, with its buttons only when `editable`.
    fn headers_form(
        form: &HeadersForm,
        details: &Details,
        editable: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> Div {
        let field = |name: &'static str, input| div().flex().flex_col().gap_1().child(label(name, theme)).child(input);
        let user = form.user.iter().enumerate().map(|(i, (k, v))| {
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(div().w(px(120.)).flex_none().child(k.clone()))
                .child(div().flex_1().min_w_0().child(v.clone()))
                .child(
                    button(("meta-remove", i), "×", Kind::Quiet, false, theme)
                        .on_click(cx.listener(move |this, _, _, cx| this.remove_metadata_row(i, cx))),
                )
        });
        section("Headers", theme)
            .child(field("Content-Type", form.content_type.clone()))
            .child(field("Cache-Control", form.cache_control.clone()))
            .child(field("Content-Disposition", form.content_disposition.clone()))
            .child(field("Content-Encoding", form.content_encoding.clone()))
            .child(field("Content-Language", form.content_language.clone()))
            .child(label("Metadata (x-amz-meta-*)", theme))
            .children(user)
            .when(!editable, |d| {
                d.child(
                    div()
                        .text_xs()
                        .text_color(theme.muted)
                        .child("Read-only: this profile's policies don't allow changing this object."),
                )
            })
            .when(editable, |d| {
                d.child(
                    div().flex().child(
                        button("meta-add", "Add metadata", Kind::Quiet, false, theme)
                            .on_click(cx.listener(|this, _, _, cx| this.add_metadata_row(cx))),
                    ),
                )
                .child(
                    div()
                        .flex()
                        .gap_1()
                        .child(
                            button(
                                "h-save",
                                if details.saving { "Saving…" } else { "Save headers" },
                                Kind::Primary,
                                details.saving,
                                theme,
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.save_headers(cx))),
                        )
                        .child(
                            button("h-reset", "Reset", Kind::Plain, details.saving, theme)
                                .on_click(cx.listener(|this, _, _, cx| this.reset_headers(cx))),
                        ),
                )
            })
    }
}

impl Root {
    /// The object's versions, when its bucket keeps them: each with its date, size and ID, and Restore and Delete as
    /// the policies allow.
    fn versions_section(&self, details: &Details, theme: &Theme, cx: &mut Context<Self>) -> Option<Div> {
        let body = match &details.versions {
            VersionsState::Hidden => return None,
            VersionsState::Loading => div().text_xs().text_color(theme.muted).child("Loading…"),
            VersionsState::Failed(why) => {
                div().text_xs().text_color(theme.danger).child(format!("Couldn't list the versions: {why}"))
            }
            VersionsState::Ready(versions) => {
                let (restore, delete) = self.version_rights(&details.key);
                let rows: Vec<_> = versions
                    .iter()
                    .enumerate()
                    .map(|(i, v)| {
                        let what = match (v.latest, v.delete_marker) {
                            (true, true) => "Deleted (delete marker)",
                            (true, false) => "Current",
                            (false, true) => "Delete marker",
                            (false, false) => "Earlier",
                        };
                        let facts =
                            if v.delete_marker { v.id.clone() } else { format!("{} · {}", size(v.size), v.id) };
                        let (to_restore, to_delete) = (v.id.clone(), v.clone());
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .debug_selector(move || format!("version {i}"))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .flex_1()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .flex()
                                            .gap_2()
                                            .text_xs()
                                            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(what))
                                            .child(
                                                div()
                                                    .text_color(theme.muted)
                                                    .child(v.modified.map_or_else(String::new, when)),
                                            ),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .font_family(MONO)
                                            .text_color(theme.muted)
                                            .truncate()
                                            .child(facts),
                                    ),
                            )
                            .when(restore && !v.latest && !v.delete_marker, |d| {
                                d.child(
                                    button(("v-restore", i), "Restore", Kind::Quiet, false, theme)
                                        .debug_selector(move || format!("restore version {i}"))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.restore_version(to_restore.clone(), cx);
                                        })),
                                )
                            })
                            .when(delete, |d| {
                                d.child(
                                    button(("v-delete", i), "Delete", Kind::Quiet, false, theme)
                                        .debug_selector(move || format!("delete version {i}"))
                                        .on_click(cx.listener(move |this, _, window, cx| {
                                            this.ask_delete_version(&to_delete, window, cx);
                                        })),
                                )
                            })
                    })
                    .collect();
                div().flex().flex_col().gap_2().children(rows)
            }
        };
        Some(section("Versions", theme).child(body))
    }
}

/// A titled section of the panel.
fn section(title: &str, theme: &Theme) -> Div {
    div().flex().flex_col().gap_2().pt_3().border_t_1().border_color(theme.border).child(
        div().text_xs().text_color(theme.muted).font_weight(gpui::FontWeight::SEMIBOLD).child(title.to_uppercase()),
    )
}

/// What the preview shows.
fn preview(details: &Details, theme: &Theme) -> Div {
    match &details.preview {
        PreviewState::Loading => div().text_xs().text_color(theme.muted).child("Loading…"),
        PreviewState::None(why) => div().text_xs().text_color(theme.muted).child(why.clone()),
        PreviewState::Image(image) => div()
            .flex()
            .justify_center()
            .rounded_md()
            .border_1()
            .border_color(theme.border)
            .bg(theme.bg)
            .p_2()
            .child(img(image.clone()).max_w_full().max_h(px(320.)).object_fit(ObjectFit::Contain)),
        PreviewState::Text(text, truncated) => div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .max_h(px(320.))
                    .overflow_hidden()
                    .rounded_md()
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.bg)
                    .p_2()
                    .font_family(MONO)
                    .text_xs()
                    .child(text.clone()),
            )
            .when(*truncated, |d| d.child(div().text_xs().text_color(theme.muted).child("Only the start is shown."))),
    }
}
