//! The open folder's rows: a header with select-all, then folders and objects drawn only near the view, then a
//! button for the next page while there's one.

use super::super::root::Root;
use super::super::theme::{MONO, Theme};
use super::super::widgets::{Kind, button, checkbox, mark};
use crate::state::browser::Row;
use crate::state::format::{name_in, size, when};
use gpui::{ClickEvent, Context, Div, Stateful, div, prelude::*, px, uniform_list};
use std::ops::Range;

/// A row's height.
const ROW: f32 = 32.;

/// The size column's width.
const SIZE_W: f32 = 96.;

/// The modified column's width.
const WHEN_W: f32 = 140.;

impl Root {
    /// The rows of the open folder, or what to do when there's none.
    pub(super) fn rows(&mut self, theme: &Theme, window: &gpui::Window, cx: &mut Context<Self>) -> Div {
        let base = div().flex().flex_col().flex_1().min_h_0();
        if self.profile.is_none() {
            return base.child(
                empty(theme, "No AWS CLI profile yet", "Add one to connect to your radosgw account.").child(
                    button("add-first", "Add profile", Kind::Primary, false, theme).on_click(cx.listener(
                        |this, _, window, cx| {
                            this.open_profiles(cx);
                            this.edit_profile(None, window, cx);
                        },
                    )),
                ),
            );
        }
        if self.browser.bucket().is_none() {
            let hint = match (self.session.is_some(), self.may_create_buckets()) {
                (false, _) => "",
                (true, true) => "Pick a bucket on the left, or create one.",
                (true, false) => "Pick a bucket on the left.",
            };
            return base.child(empty(theme, "No bucket open", hint));
        }
        if !self.open_bucket_listable() {
            let bucket = self.browser.bucket().unwrap_or_default();
            return base.child(empty(
                theme,
                "No access to this bucket",
                &format!(
                    "This profile's policies don't let it see what's in {bucket}. Ask your platform team if you need it."
                ),
            ));
        }
        let count = self.browser.len();
        let all = count > 0 && self.browser.selection().len() == count;
        let dated = !super::compact(window);
        let header = div()
            .debug_selector(|| "rows header".to_owned())
            .flex()
            .items_center()
            .gap_3()
            .h(px(30.))
            .px_4()
            .border_b_1()
            .border_color(theme.border)
            .text_xs()
            .text_color(theme.muted)
            .child(checkbox("select-all", all, theme).on_click(cx.listener(|this, _, _, cx| {
                this.browser.toggle_all();
                this.sync_details(cx);
                cx.notify();
            })))
            .child(div().flex_1().child("Name"))
            .child(div().w(px(SIZE_W)).text_right().child("Size"))
            .when(dated, |d| d.child(div().w(px(WHEN_W)).child("Modified")));
        let body = if count == 0 && !self.browser.loading() {
            empty(theme, "This folder is empty", "Drop files here, or use Upload, to add some.")
        } else {
            div().flex_1().min_h_0().child(
                uniform_list(
                    "rows",
                    count,
                    cx.processor(|this, range: Range<usize>, window, cx| this.row_range(range, window, cx)),
                )
                .track_scroll(&self.list_scroll)
                .size_full(),
            )
        };
        let footer = (self.browser.has_more() || self.browser.loading()).then(|| {
            div().flex().justify_center().py_2().border_t_1().border_color(theme.border).child(
                button(
                    "more",
                    if self.browser.loading() { "Loading…" } else { "Load more" },
                    Kind::Plain,
                    self.browser.loading(),
                    theme,
                )
                .on_click(cx.listener(|this, _, _, cx| this.load_more(cx))),
            )
        });
        base.child(header).child(body).children(footer)
    }

    /// The rows in `range`, for the list.
    fn row_range(
        &mut self,
        range: Range<usize>,
        window: &mut gpui::Window,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let theme = super::super::theme::of(window);
        range
            .filter_map(|index| {
                self.browser.row(index).cloned().map(|row| self.row(index, &row, &theme, !super::compact(window), cx))
            })
            .collect()
    }

    /// Row `index`, its Modified column only when `dated`.
    fn row(&self, index: usize, row: &Row, theme: &Theme, dated: bool, cx: &mut Context<Self>) -> Stateful<Div> {
        let selected = self.browser.is_selected(row.key());
        let name = name_in(row.key(), self.browser.prefix()).to_owned();
        let (size_text, when_text) = match row {
            Row::Folder(_) => (String::new(), String::new()),
            Row::Object(o) => (size(o.size), o.modified.map(when).unwrap_or_default()),
        };
        let folder = matches!(row, Row::Folder(_));
        let (selector, name_selector) = {
            let (a, b) = (name.clone(), name.clone());
            (move || format!("row {a}"), move || format!("name {b}"))
        };
        div()
            .id(("row", index))
            .debug_selector(selector)
            .w_full()
            .flex()
            .items_center()
            .gap_3()
            .h(px(ROW))
            .px_4()
            .cursor_pointer()
            .when(selected, |d| d.bg(theme.selected))
            .when(!selected, |d| d.hover(|s| s.bg(theme.hover)))
            .child(checkbox(("check", index), selected, theme).on_click(cx.listener(move |this, _, _, cx| {
                cx.stop_propagation();
                this.toggle_row(index, cx);
            })))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_1()
                    .min_w_0()
                    .debug_selector(name_selector)
                    .child(mark(folder, theme))
                    .child(div().truncate().child(name)),
            )
            .child(
                div().w(px(SIZE_W)).text_right().font_family(MONO).text_xs().text_color(theme.muted).child(size_text),
            )
            .when(dated, |d| d.child(div().w(px(WHEN_W)).text_xs().text_color(theme.muted).child(when_text)))
            .on_click(cx.listener(move |this, event: &ClickEvent, window, cx| {
                let modifiers = event.modifiers();
                if event.click_count() >= 2 && !folder && !modifiers.secondary() && !modifiers.shift {
                    this.download_selected(cx);
                    return;
                }
                this.click_row(index, modifiers.secondary(), modifiers.shift, window, cx);
            }))
    }
}

/// A centered message for an empty area, with what to do about it.
fn empty(theme: &Theme, title: &str, hint: &str) -> Div {
    div()
        .flex()
        .flex_col()
        .flex_1()
        .items_center()
        .justify_center()
        .gap_2()
        .child(div().text_base().font_weight(gpui::FontWeight::SEMIBOLD).child(title.to_owned()))
        .child(div().text_color(theme.muted).child(hint.to_owned()))
}
