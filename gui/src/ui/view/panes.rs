//! The top bar (profile and refresh), the buckets on the left, and the open folder's path and toolbar.

use super::super::rights;
use super::super::root::Root;
use super::super::theme::Theme;
use super::super::widgets::{Kind, button, mark};
use crate::state::format::breadcrumbs;
use gpui::{Context, Div, Stateful, Window, deferred, div, prelude::*, px};

impl Root {
    /// The top bar.
    pub(super) fn topbar(&mut self, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let profile = self.profile.clone().unwrap_or_else(|| "No profile".to_owned());
        let endpoint = self.session.as_ref().and_then(|s| s.endpoint()).map(str::to_owned);
        let state = match (&self.session, &self.profile) {
            (None, Some(_)) if self.buckets_loading => Some("connecting…"),
            (None, Some(_)) => Some("not connected"),
            (Some(_), _) | (None, None) => None,
        };
        div()
            .flex()
            .items_center()
            .gap_3()
            .h(px(46.))
            .px_4()
            .bg(theme.panel)
            .border_b_1()
            .border_color(theme.border)
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).text_base().child("rs3manager"))
            .child(
                // The dropdown hangs under the button, drawn after (over) everything else in the window.
                div()
                    .relative()
                    .child(
                        button("profile", format!("{profile}  ▾"), Kind::Plain, false, theme)
                            .on_click(cx.listener(|this, _, window, cx| this.open_profile_menu(window, cx))),
                    )
                    .children(
                        self.profile_menu(theme, cx)
                            .map(|menu| deferred(div().absolute().top(px(32.)).left_0().child(menu)).with_priority(1)),
                    ),
            )
            .children(endpoint.map(|e| div().text_xs().text_color(theme.muted).truncate().child(e)))
            .children(state.map(|s| div().text_xs().text_color(theme.muted).child(s)))
            .children(self.identity().map(|who| {
                div().text_xs().text_color(theme.muted).truncate().debug_selector(|| "identity".to_owned()).child(who)
            }))
            .child(div().flex_1())
            .child(
                button("refresh", "Refresh", Kind::Quiet, self.session.is_none(), theme)
                    .on_click(cx.listener(|this, _, _, cx| this.refresh(cx))),
            )
    }

    /// The buckets list.
    pub(super) fn sidebar(&mut self, theme: &Theme, window: &Window, cx: &mut Context<Self>) -> Div {
        let open = self.browser.bucket().map(str::to_owned);
        let rows = self.buckets.iter().enumerate().map(|(i, bucket)| {
            let name = bucket.name.clone();
            let selected = open.as_deref() == Some(name.as_str());
            let listable = bucket.listable;
            div()
                .id(("bucket", i))
                .flex()
                .items_center()
                .gap_2()
                .px_3()
                .h(px(30.))
                .rounded_md()
                .cursor_pointer()
                .when(selected, |d| d.bg(theme.selected))
                .when(!selected, |d| d.hover(|s| s.bg(theme.hover)))
                .child(mark(true, theme))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .when(!listable, |d| d.text_color(theme.muted))
                        .child(name.clone()),
                )
                .when(!listable, |d| d.child(div().flex_none().text_xs().text_color(theme.muted).child("no access")))
                .debug_selector(|| format!("bucket {name}"))
                .on_click(cx.listener(move |this, _, window, cx| this.open_bucket(&name, window, cx)))
        });
        let empty = (self.session.is_some() && !self.buckets_loading && self.buckets.is_empty()).then(|| {
            let text = if self.buckets_failed { "Couldn't list the buckets" } else { "No buckets yet" };
            div()
                .px_3()
                .py_2()
                .text_xs()
                .text_color(theme.muted)
                .child(text)
                .debug_selector(|| "buckets empty".to_owned())
        });
        div()
            .flex()
            .flex_col()
            .w(px(if super::compact(window) { 200. } else { 240. }))
            .flex_none()
            .bg(theme.panel)
            .border_r_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_3()
                    .h(px(40.))
                    .child(
                        div()
                            .text_xs()
                            .text_color(theme.muted)
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child("BUCKETS"),
                    )
                    .when(self.may_create_buckets(), |d| {
                        d.child(
                            button("new-bucket", "New", Kind::Quiet, self.session.is_none(), theme)
                                .on_click(cx.listener(|this, _, window, cx| this.ask_create_bucket(window, cx))),
                        )
                    }),
            )
            .child(
                div()
                    .id("buckets")
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .px_2()
                    .flex_1()
                    .overflow_y_scroll()
                    .children(rows)
                    .children(empty)
                    .when(self.buckets_loading, |d| {
                        d.child(div().px_3().py_2().text_xs().text_color(theme.muted).child("Loading…"))
                    }),
            )
    }

    /// The path of the open folder, the filter, and what can be done there.
    pub(super) fn toolbar(&mut self, theme: &Theme, _window: &mut Window, cx: &mut Context<Self>) -> Div {
        let Some(bucket) = self.browser.bucket().map(str::to_owned) else {
            return div();
        };
        let prefix = self.browser.prefix().to_owned();
        let selected = self.browser.selection().len();
        let listable = self.open_bucket_listable();
        let mut path = div().flex().items_center().gap_1().min_w_0().overflow_hidden().child(
            crumb("crumb-top", &bucket, prefix.is_empty(), theme).on_click(cx.listener({
                let bucket = bucket.clone();
                move |this, _, window, cx| this.open_folder(Some(bucket.clone()), String::new(), window, cx)
            })),
        );
        for (i, (name, target)) in breadcrumbs(&prefix).into_iter().enumerate() {
            let (target, last) = (target.to_owned(), target == prefix);
            let bucket = bucket.clone();
            path = path.child(div().text_color(theme.muted).child("/")).child(
                crumb(("crumb", i), name, last, theme).on_click(cx.listener(move |this, _, window, cx| {
                    this.open_folder(Some(bucket.clone()), target.clone(), window, cx);
                })),
            );
        }
        let none = selected == 0;
        let (write, read, delete, rename) = (
            self.may_write_here(),
            self.may_on_selection(rights::READ),
            self.may_on_selection(rights::DELETE),
            self.may_on_selection(rights::RENAME),
        );
        // Wraps onto more lines in a narrow window rather than pushing buttons out of sight.
        let actions = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_1()
            .when(write, |d| {
                d.child(
                    button("upload", "Upload files", Kind::Primary, false, theme)
                        .on_click(cx.listener(|this, _, _, cx| this.pick_uploads(false, cx))),
                )
                .child(
                    button("upload-dir", "Upload folder", Kind::Plain, false, theme)
                        .on_click(cx.listener(|this, _, _, cx| this.pick_uploads(true, cx))),
                )
                .child(
                    button("new-folder", "New folder", Kind::Plain, false, theme)
                        .on_click(cx.listener(|this, _, window, cx| this.ask_create_folder(window, cx))),
                )
                .child(div().w(px(8.)))
            })
            // Disabled without a selection, hidden when the policies deny it for what's selected.
            .when(none || read, |d| {
                d.child(
                    button("download", "Download", Kind::Plain, none, theme)
                        .on_click(cx.listener(|this, _, _, cx| this.download_selected(cx))),
                )
            })
            .when(none || rename, |d| {
                d.child(
                    button("rename", "Rename", Kind::Plain, selected != 1, theme)
                        .on_click(cx.listener(|this, _, window, cx| this.ask_rename(window, cx))),
                )
            })
            .when(none || delete, |d| {
                d.child(
                    button("delete", "Delete", Kind::Danger, none, theme)
                        .on_click(cx.listener(|this, _, window, cx| this.ask_delete_selected(window, cx))),
                )
            });
        let bucket_actions = prefix.is_empty().then(|| self.bucket_actions(&bucket, theme, cx)).flatten();
        div()
            .flex()
            .flex_col()
            .gap_2()
            .px_4()
            .py_3()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(path.flex_1())
                    .child(div().w(px(200.)).flex_none().child(self.filter.clone())),
            )
            .when(listable, |d| {
                d.child(actions.children(bucket_actions.map(|b| div().flex_1().flex().justify_end().child(b))))
            })
    }

    /// Deleting the open bucket, as far as the policies allow; `None` when they allow neither way.
    fn bucket_actions(&self, bucket: &str, theme: &Theme, cx: &mut Context<Self>) -> Option<Div> {
        let (delete, empty) = (self.may_delete_bucket(bucket, false), self.may_delete_bucket(bucket, true));
        let (a, b) = (bucket.to_owned(), bucket.to_owned());
        delete.then(|| {
            div()
                .flex()
                .gap_1()
                .child(
                    button("delete-bucket", "Delete bucket", Kind::Quiet, false, theme).on_click(
                        cx.listener(move |this, _, window, cx| this.ask_delete_bucket(&a, false, window, cx)),
                    ),
                )
                .when(empty, |d| {
                    d.child(
                        button("empty-bucket", "Empty and delete", Kind::Quiet, false, theme).on_click(
                            cx.listener(move |this, _, window, cx| this.ask_delete_bucket(&b, true, window, cx)),
                        ),
                    )
                })
        })
    }
}

/// A folder in the path, the last one (the open folder) bold and not a link.
fn crumb(id: impl Into<gpui::ElementId>, name: &str, last: bool, theme: &Theme) -> Stateful<Div> {
    // Folders above shrink first; the open one keeps its name readable.
    let base = div().id(id.into()).px_1().rounded_sm().min_w(px(24.)).truncate().child(name.to_owned());
    if last {
        base.font_weight(gpui::FontWeight::SEMIBOLD)
    } else {
        base.text_color(theme.accent).cursor_pointer().hover(|s| s.bg(theme.hover))
    }
}
