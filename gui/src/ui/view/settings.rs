//! A bucket's settings dialog: its tabs, the open tab's rows or rule cards, and Save, Reset and Remove all. A tab
//! the profile may not write shows its values as text, saying why.

use super::super::root::Root;
use super::super::settings::{CorsFields, Field, Fields, RuleFields, SettingsDialog, TabState, TagFields};
use super::super::theme::{MONO, Theme};
use super::super::widgets::{Kind, button, checkbox, label};
use super::overlays::{card, footer, heading};
use crate::state::settings::Tab;
use gpui::{AnyElement, Context, Div, div, prelude::*, px};
use rs3manager_core::s3::settings::CORS_METHODS;

impl Root {
    /// The dialog of `dialog.bucket`.
    pub(super) fn settings_dialog(&self, dialog: &SettingsDialog, theme: &Theme, cx: &mut Context<Self>) -> Div {
        let tab = dialog.tab;
        let editable = self.may_edit_settings(tab, &dialog.bucket);
        let tabs = self.settings_tabs(&dialog.bucket).into_iter().map(|t| {
            let kind = if t == tab { Kind::Primary } else { Kind::Plain };
            button(t.label(), t.label(), kind, false, theme)
                .on_click(cx.listener(move |this, _, _, cx| this.show_settings_tab(t, cx)))
        });
        let tabs: Vec<_> = tabs.collect();
        let body = match &dialog.state {
            TabState::Loading => div().text_xs().text_color(theme.muted).child("Loading…"),
            TabState::Failed(why) => {
                div().text_xs().text_color(theme.danger).child(format!("Couldn't read the {}: {why}", tab.noun()))
            }
            TabState::Ready { fields, .. } => tab_body(fields, editable, theme, cx),
        };
        let ready = matches!(dialog.state, TabState::Ready { .. });
        let empty = match &dialog.state {
            TabState::Ready { loaded, .. } => loaded.is_empty(),
            _ => true,
        };
        let busy = dialog.saving;
        let buttons = if dialog.confirm_remove {
            footer()
                .items_center()
                .child(div().flex_1().text_xs().text_color(theme.danger).child(format!(
                    "Remove every one of the {} of {}? It can't be undone.",
                    tab.noun(),
                    dialog.bucket
                )))
                .child(
                    button("settings-keep", "Keep", Kind::Plain, false, theme)
                        .on_click(cx.listener(|this, _, _, cx| this.keep_settings(cx))),
                )
                .child(
                    button("settings-remove", "Remove", Kind::Danger, busy, theme)
                        .on_click(cx.listener(|this, _, _, cx| this.remove_settings(cx))),
                )
        } else {
            footer()
                .when(editable && ready, |d| {
                    d.child(
                        button("settings-remove-all", "Remove all", Kind::Danger, busy || empty, theme)
                            .on_click(cx.listener(|this, _, _, cx| this.ask_remove_settings(cx))),
                    )
                    .child(div().flex_1())
                    .child(
                        button("settings-add", add_label(tab), Kind::Plain, busy, theme)
                            .on_click(cx.listener(|this, _, _, cx| this.add_settings_row(cx))),
                    )
                    .child(
                        button("settings-reset", "Reset", Kind::Plain, busy, theme)
                            .on_click(cx.listener(|this, _, _, cx| this.reset_settings(cx))),
                    )
                })
                .child(
                    button("settings-close", "Close", Kind::Plain, false, theme)
                        .on_click(cx.listener(|this, _, window, cx| this.close_modal(window, cx))),
                )
                .when(editable && ready, |d| {
                    d.child(
                        button("settings-save", if busy { "Saving…" } else { "Save" }, Kind::Primary, busy, theme)
                            .on_click(cx.listener(|this, _, _, cx| this.save_settings(cx))),
                    )
                })
        };
        card(theme, 680.)
            .child(heading(format!("Settings of {}", dialog.bucket).into()))
            .child(div().flex().gap_1().children(tabs))
            .child(div().text_xs().text_color(theme.muted).child(about(tab)))
            .child(div().id("settings-body").flex().flex_col().gap_2().max_h(px(420.)).overflow_y_scroll().child(body))
            .when(!editable, |d| {
                d.child(div().text_xs().text_color(theme.muted).child(format!(
                    "Read-only: this profile's policies don't allow {} on this bucket.",
                    tab.write_action()
                )))
            })
            .children(dialog.error.clone().map(|e| {
                div().text_xs().text_color(theme.danger).debug_selector(|| "settings error".to_owned()).child(e)
            }))
            .child(buttons)
    }
}

/// What tab `tab` does, under the tabs.
fn about(tab: Tab) -> &'static str {
    match tab {
        Tab::Tags => "Key and value labels on the bucket, e.g. for cost reports. A bucket has at most 50.",
        Tab::Lifecycle => {
            "Rules the gateway runs daily on the keys under a prefix: expiring objects, old versions and unfinished \
             uploads. Rules set elsewhere with more than that are shown and kept as they are."
        }
        Tab::Cors => "Which web origins may call the bucket from a browser, with which methods and headers.",
    }
}

/// The button adding a row to tab `tab`.
fn add_label(tab: Tab) -> &'static str {
    match tab {
        Tab::Tags => "Add tag",
        Tab::Lifecycle | Tab::Cors => "Add rule",
    }
}

/// The open tab's rows or cards.
fn tab_body(fields: &Fields, editable: bool, theme: &Theme, cx: &mut Context<Root>) -> Div {
    let (count, none) = match fields {
        Fields::Tags(rows) => (rows.len(), "No tags."),
        Fields::Lifecycle(cards) => (cards.len(), "No lifecycle rules."),
        Fields::Cors(cards) => (cards.len(), "No CORS rules."),
    };
    let items: Vec<AnyElement> = match fields {
        Fields::Tags(rows) => rows.iter().enumerate().map(|(i, r)| tag_row(i, r, editable, theme, cx)).collect(),
        Fields::Lifecycle(cards) => {
            cards.iter().enumerate().map(|(i, c)| rule_card(i, c, editable, theme, cx)).collect()
        }
        Fields::Cors(cards) => cards.iter().enumerate().map(|(i, c)| cors_card(i, c, editable, theme, cx)).collect(),
    };
    div()
        .flex()
        .flex_col()
        .gap_2()
        .children(items)
        .when(count == 0, |d| d.child(div().text_xs().text_color(theme.muted).child(none)))
}

/// `field` as typed into when `editable`, else its text.
fn value(field: &Field, editable: bool, theme: &Theme, cx: &Context<Root>) -> AnyElement {
    if editable {
        field.clone().into_any_element()
    } else {
        let text = field.read(cx).text().to_owned();
        div()
            .py_1()
            .font_family(MONO)
            .text_xs()
            .when(text.is_empty(), |d| d.text_color(theme.muted).child("—"))
            .when(!text.is_empty(), |d| d.child(text))
            .into_any_element()
    }
}

/// A labelled field, `width` wide, or taking the room left when `None`.
fn labelled(name: &'static str, field: AnyElement, width: Option<f32>, theme: &Theme) -> Div {
    let base = div().flex().flex_col().gap_1().min_w_0().child(label(name, theme)).child(field);
    match width {
        Some(w) => base.w(px(w)).flex_none(),
        None => base.flex_1(),
    }
}

/// The button removing row or card `i`, when `editable`.
fn remove(i: usize, editable: bool, theme: &Theme, cx: &mut Context<Root>) -> Option<gpui::Stateful<Div>> {
    editable.then(|| {
        button(("settings-row-remove", i), "×", Kind::Quiet, false, theme)
            .on_click(cx.listener(move |this, _, _, cx| this.remove_settings_row(i, cx)))
    })
}

/// A rule's box.
fn rule_box(theme: &Theme) -> Div {
    div().flex().flex_col().gap_2().p_3().rounded_md().border_1().border_color(theme.border).bg(theme.bg)
}

/// Tag `i`: its key and value.
fn tag_row(i: usize, row: &TagFields, editable: bool, theme: &Theme, cx: &mut Context<Root>) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(div().w(px(220.)).flex_none().child(value(&row.key, editable, theme, cx)))
        .child(div().flex_1().min_w_0().child(value(&row.value, editable, theme, cx)))
        .children(remove(i, editable, theme, cx))
        .into_any_element()
}

/// Lifecycle rule `i`: its fields, or what it holds when kept as it came.
fn rule_card(i: usize, card: &RuleFields, editable: bool, theme: &Theme, cx: &mut Context<Root>) -> AnyElement {
    if let Some(rule) = &card.kept {
        let extras = rule.kept.as_ref().map_or_default(|k| k.extras.join(", "));
        let prefix = if rule.prefix.is_empty() { "every key".to_owned() } else { rule.prefix.clone() };
        return rule_box(theme)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(div().flex_1().font_family(MONO).child(rule.id.clone()))
                    .child(div().text_xs().text_color(theme.muted).child(if rule.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    }))
                    .children(remove(i, editable, theme, cx)),
            )
            .child(div().text_xs().font_family(MONO).text_color(theme.muted).child(prefix))
            .child(
                div()
                    .text_xs()
                    .text_color(theme.muted)
                    .child(format!("Holds {extras}, which this window doesn't edit: saved back as it is.")),
            )
            .into_any_element();
    }
    let enabled = checkbox(("rule-enabled", i), card.enabled, theme)
        .debug_selector(move || format!("rule {i} enabled"))
        .when(editable, |c| c.on_click(cx.listener(move |this, _, _, cx| this.toggle_rule_enabled(i, cx))));
    rule_box(theme)
        .child(
            div()
                .flex()
                .items_end()
                .gap_2()
                .child(labelled("ID", value(&card.id, editable, theme, cx), Some(180.), theme))
                .child(labelled("Prefix", value(&card.prefix, editable, theme, cx), None, theme))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1()
                        .h(px(28.))
                        .child(enabled)
                        .child(div().text_xs().child("Enabled")),
                )
                .children(remove(i, editable, theme, cx)),
        )
        .child(
            div()
                .flex()
                .gap_2()
                .child(labelled("Expire objects after (days)", value(&card.expire, editable, theme, cx), None, theme))
                .child(labelled(
                    "Delete noncurrent versions after",
                    value(&card.noncurrent, editable, theme, cx),
                    None,
                    theme,
                ))
                .child(labelled(
                    "Abort incomplete uploads after",
                    value(&card.abort, editable, theme, cx),
                    None,
                    theme,
                )),
        )
        .into_any_element()
}

/// CORS rule `i`: origins, methods, headers and max age.
fn cors_card(i: usize, card: &CorsFields, editable: bool, theme: &Theme, cx: &mut Context<Root>) -> AnyElement {
    let methods = CORS_METHODS.iter().enumerate().map(|(m, name)| {
        let kind = if card.methods[m] { Kind::Primary } else { Kind::Plain };
        button(("cors-method", i * CORS_METHODS.len() + m), *name, kind, !editable, theme)
            .debug_selector(move || format!("rule {i} {name}"))
            .when(editable, |b| b.on_click(cx.listener(move |this, _, _, cx| this.toggle_cors_method(i, m, cx))))
    });
    let methods: Vec<_> = methods.collect();
    rule_box(theme)
        .child(
            div()
                .flex()
                .items_end()
                .gap_2()
                .child(labelled("Origins (comma-separated)", value(&card.origins, editable, theme, cx), None, theme))
                .children(remove(i, editable, theme, cx)),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap_1()
                .child(label("Methods", theme))
                .child(div().flex().gap_1().children(methods)),
        )
        .child(
            div()
                .flex()
                .gap_2()
                .child(labelled("Allowed headers", value(&card.allowed_headers, editable, theme, cx), None, theme))
                .child(labelled("Exposed headers", value(&card.expose_headers, editable, theme, cx), None, theme))
                .child(labelled("Max age (seconds)", value(&card.max_age, editable, theme, cx), Some(140.), theme)),
        )
        .into_any_element()
}
