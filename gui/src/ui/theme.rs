//! Colors and fonts, light or dark as the desktop is.

use gpui::{Rgba, Window, WindowAppearance, rgba};

/// The UI font, built in (IBM Plex Sans): GPUI finds no system font to make bold with on many Linux desktops.
pub const SANS: &str = "IBM Plex Sans";

/// The font for keys, sizes and previews of text (Lilex), built in.
pub const MONO: &str = "Lilex";

/// The colors the window draws with.
#[derive(Debug, Clone, Copy)]
pub struct Theme {
    /// The window's background.
    pub bg: Rgba,
    /// Side and top panels.
    pub panel: Rgba,
    /// Dialogs and menus over the rest.
    pub raised: Rgba,
    /// Lines between areas and around fields.
    pub border: Rgba,
    /// Text.
    pub text: Rgba,
    /// Secondary text: sizes, dates, hints.
    pub muted: Rgba,
    /// The main action and focus.
    pub accent: Rgba,
    /// Text on [`Theme::accent`].
    pub on_accent: Rgba,
    /// Destructive actions and errors.
    pub danger: Rgba,
    /// A row or button under the pointer.
    pub hover: Rgba,
    /// A selected row.
    pub selected: Rgba,
    /// A folder's mark.
    pub folder: Rgba,
    /// A text field's background.
    pub input: Rgba,
    /// Selected text in a field.
    pub text_selection: Rgba,
    /// What dims the window under a dialog.
    pub scrim: Rgba,
}

/// The light theme.
const LIGHT: Theme = Theme {
    bg: Rgba { r: 1., g: 1., b: 1., a: 1. },
    panel: Rgba { r: 0.965, g: 0.969, b: 0.976, a: 1. },
    raised: Rgba { r: 1., g: 1., b: 1., a: 1. },
    border: Rgba { r: 0.86, g: 0.87, b: 0.89, a: 1. },
    text: Rgba { r: 0.11, g: 0.12, b: 0.14, a: 1. },
    muted: Rgba { r: 0.42, g: 0.45, b: 0.5, a: 1. },
    accent: Rgba { r: 0.13, g: 0.4, b: 0.86, a: 1. },
    on_accent: Rgba { r: 1., g: 1., b: 1., a: 1. },
    danger: Rgba { r: 0.8, g: 0.18, b: 0.2, a: 1. },
    hover: Rgba { r: 0.93, g: 0.94, b: 0.96, a: 1. },
    selected: Rgba { r: 0.87, g: 0.92, b: 1., a: 1. },
    folder: Rgba { r: 0.93, g: 0.66, b: 0.18, a: 1. },
    input: Rgba { r: 1., g: 1., b: 1., a: 1. },
    text_selection: Rgba { r: 0.13, g: 0.4, b: 0.86, a: 0.25 },
    scrim: Rgba { r: 0., g: 0., b: 0., a: 0.35 },
};

/// The dark theme.
const DARK: Theme = Theme {
    bg: Rgba { r: 0.11, g: 0.115, b: 0.13, a: 1. },
    panel: Rgba { r: 0.14, g: 0.145, b: 0.165, a: 1. },
    raised: Rgba { r: 0.17, g: 0.175, b: 0.2, a: 1. },
    border: Rgba { r: 0.25, g: 0.26, b: 0.29, a: 1. },
    text: Rgba { r: 0.9, g: 0.91, b: 0.93, a: 1. },
    muted: Rgba { r: 0.6, g: 0.62, b: 0.67, a: 1. },
    accent: Rgba { r: 0.35, g: 0.6, b: 1., a: 1. },
    on_accent: Rgba { r: 0.05, g: 0.07, b: 0.12, a: 1. },
    danger: Rgba { r: 0.95, g: 0.4, b: 0.4, a: 1. },
    hover: Rgba { r: 0.19, g: 0.2, b: 0.23, a: 1. },
    selected: Rgba { r: 0.17, g: 0.26, b: 0.42, a: 1. },
    folder: Rgba { r: 0.95, g: 0.7, b: 0.3, a: 1. },
    input: Rgba { r: 0.12, g: 0.125, b: 0.14, a: 1. },
    text_selection: Rgba { r: 0.35, g: 0.6, b: 1., a: 0.35 },
    scrim: Rgba { r: 0., g: 0., b: 0., a: 0.55 },
};

/// The theme for `window`'s appearance.
pub fn of(window: &Window) -> Theme {
    match window.appearance() {
        WindowAppearance::Dark | WindowAppearance::VibrantDark => DARK,
        WindowAppearance::Light | WindowAppearance::VibrantLight => LIGHT,
    }
}

/// A color with its alpha scaled by `alpha`: hover and pressed shades of a button's own color.
pub fn faded(color: Rgba, alpha: f32) -> Rgba {
    rgba(u32::from_be_bytes([channel(color.r), channel(color.g), channel(color.b), channel(color.a * alpha)]))
}

/// A 0..=1 color channel as a byte.
#[expect(clippy::cast_possible_truncation, clippy::cast_sign_loss, reason = "clamped to 0..=255 first")]
fn channel(value: f32) -> u8 {
    (value.clamp(0., 1.) * 255.).round() as u8
}
