//! Buttons, labels and the small marks rows are drawn with, in the theme's colors.

use super::theme::{Theme, faded};
use gpui::{Div, ElementId, SharedString, Stateful, div, prelude::*, px};

/// How a button reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// The dialog's or area's main action.
    Primary,
    /// Any other action.
    Plain,
    /// An action that deletes.
    Danger,
    /// No border or fill until hovered: in toolbars and rows.
    Quiet,
}

/// A button showing `label`; add its `on_click`. A `disabled` one is dimmed and ignores the pointer.
pub fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    kind: Kind,
    disabled: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let (bg, fg, border) = match kind {
        Kind::Primary => (theme.accent, theme.on_accent, theme.accent),
        Kind::Plain => (theme.raised, theme.text, theme.border),
        Kind::Danger => (theme.raised, theme.danger, theme.border),
        Kind::Quiet => (faded(theme.raised, 0.), theme.text, faded(theme.border, 0.)),
    };
    let label = label.into();
    // Only test builds call it, so release builds format nothing.
    let selector = {
        let label = label.clone();
        move || format!("button {label}")
    };
    let hover = match kind {
        Kind::Primary => faded(theme.accent, 0.85),
        _ => theme.hover,
    };
    let base = div()
        .id(id.into())
        .flex()
        .items_center()
        .justify_center()
        .px_3()
        .h(px(28.))
        .rounded_md()
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_color(fg)
        .text_sm()
        .whitespace_nowrap()
        .debug_selector(selector)
        .child(label);
    if disabled { base.opacity(0.45) } else { base.cursor_pointer().hover(move |s| s.bg(hover)) }
}

/// A form label above its field.
pub fn label(text: impl Into<SharedString>, theme: &Theme) -> Div {
    div().text_xs().text_color(theme.muted).child(text.into())
}

/// The mark before a row's name: a folder's filled tab, an object's outline.
pub fn mark(folder: bool, theme: &Theme) -> Div {
    let base = div().w(px(14.)).h(px(11.)).rounded(px(2.)).flex_none();
    if folder { base.bg(theme.folder) } else { base.border_1().border_color(theme.muted) }
}

/// A checkbox, checked or not; add its `on_click`.
pub fn checkbox(id: impl Into<ElementId>, checked: bool, theme: &Theme) -> Stateful<Div> {
    let base = div()
        .id(id.into())
        .flex_none()
        .w(px(14.))
        .h(px(14.))
        .rounded(px(3.))
        .border_1()
        .cursor_pointer()
        .flex()
        .items_center()
        .justify_center();
    if checked {
        base.bg(theme.accent)
            .border_color(theme.accent)
            .child(div().w(px(6.)).h(px(6.)).rounded(px(1.)).bg(theme.on_accent))
    } else {
        base.border_color(theme.muted)
    }
}
