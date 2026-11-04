//! The window, drawn with GPUI: profiles on top, buckets on the left, the open folder in the middle, the selected
//! object's details on the right, transfers at the bottom, and dialogs over it all.
//!
//! - [`root`]: the window's state and what every action does to it.
//! - `browse`: buckets, folders, objects: listing, creating, renaming, deleting, metadata, previews.
//! - `transfer`: uploads (also dropped files) and downloads, queued and running on the session.
//! - `profiles`: the AWS CLI profiles dialog.
//! - `rights`: what the profile's policies allow, and so what's shown.
//! - `share`: download links to an object, with an expiry.
//! - `view`: drawing each area.
//! - [`input`]: text fields.
//! - [`theme`]: colors and fonts.

mod browse;
pub mod input;
mod profiles;
mod rights;
mod root;
mod share;
pub mod theme;
mod transfer;
mod view;
mod widgets;

#[cfg(test)]
mod tests;

use anyhow::Context as _;
use gpui::{App, Bounds, KeyBinding, WindowBounds, WindowOptions, actions, prelude::*, px, size};
use root::Root;
use std::borrow::Cow;

actions!(
    r3manager,
    [
        /// Confirms the dialog open.
        Submit,
        /// Closes the dialog open, or clears the selection.
        Cancel,
        /// Lists the buckets and the open folder again.
        Refresh,
        /// Asks to delete the selected rows.
        DeleteSelected,
        /// Asks for a new name for the selected row.
        RenameSelected,
        /// Selects every row shown, or none.
        SelectAllRows,
        /// Moves to the previous entry of the open list (the profile dropdown).
        MenuUp,
        /// Moves to the next entry of the open list.
        MenuDown,
        /// Opens the folder above.
        GoUp,
        /// Puts the cursor in the filter.
        FocusFilter,
        /// Quits.
        Quit,
    ]
);

/// The window's application id on Linux, which desktops match their launcher entry by.
const APP_ID: &str = "r3manager";

/// The built-in fonts (licenses beside them in `gui/fonts/`).
const FONTS: [&[u8]; 6] = [
    include_bytes!("../../fonts/ibm-plex-sans/IBMPlexSans-Regular.ttf"),
    include_bytes!("../../fonts/ibm-plex-sans/IBMPlexSans-Italic.ttf"),
    include_bytes!("../../fonts/ibm-plex-sans/IBMPlexSans-SemiBold.ttf"),
    include_bytes!("../../fonts/ibm-plex-sans/IBMPlexSans-SemiBoldItalic.ttf"),
    include_bytes!("../../fonts/lilex/Lilex-Regular.ttf"),
    include_bytes!("../../fonts/lilex/Lilex-Bold.ttf"),
];

/// The keys: text fields' editing, dialogs' enter and escape, and the browser's shortcuts. A field's own bindings
/// win while it has focus, so typing never deletes rows.
pub(crate) fn bind_keys(cx: &mut App) {
    use input::{Backspace, Copy, Cut, Delete, End, Home, Left, Paste, Right, SelectAll, SelectLeft, SelectRight};
    let field = Some("TextInput");
    let browser = Some("Browser");
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, field),
        KeyBinding::new("delete", Delete, field),
        KeyBinding::new("left", Left, field),
        KeyBinding::new("right", Right, field),
        KeyBinding::new("shift-left", SelectLeft, field),
        KeyBinding::new("shift-right", SelectRight, field),
        KeyBinding::new("secondary-a", SelectAll, field),
        KeyBinding::new("secondary-v", Paste, field),
        KeyBinding::new("secondary-c", Copy, field),
        KeyBinding::new("secondary-x", Cut, field),
        KeyBinding::new("home", Home, field),
        KeyBinding::new("end", End, field),
        KeyBinding::new("enter", Submit, Some("Dialog")),
        KeyBinding::new("up", MenuUp, Some("Dialog")),
        KeyBinding::new("down", MenuDown, Some("Dialog")),
        KeyBinding::new("escape", Cancel, None),
        KeyBinding::new("f5", Refresh, browser),
        KeyBinding::new("secondary-r", Refresh, browser),
        KeyBinding::new("delete", DeleteSelected, browser),
        KeyBinding::new("f2", RenameSelected, browser),
        KeyBinding::new("secondary-a", SelectAllRows, browser),
        KeyBinding::new("backspace", GoUp, browser),
        KeyBinding::new("alt-up", GoUp, browser),
        KeyBinding::new("secondary-f", FocusFilter, browser),
        KeyBinding::new("secondary-q", Quit, None),
    ]);
}

/// GPUI's app for this platform. GPUI panics when it can't connect to the window system (no display, a compositor
/// that refuses it); that's reported as an error instead of a panic's backtrace.
fn application() -> anyhow::Result<gpui::Application> {
    let hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(|_| {}));
    let app = std::panic::catch_unwind(gpui_platform::application);
    std::panic::set_hook(hook);
    app.map_err(|_| anyhow::anyhow!("couldn't connect to the window system: run r3manager from a desktop session"))
}

/// Opens the window and runs until it's closed.
pub fn run() -> anyhow::Result<()> {
    let engine = r3manager_core::s3::Engine::new().context("couldn't start")?;
    let files = r3manager_core::profiles::Files::from_env().context("couldn't find the AWS CLI's files")?;
    application()?.run(move |cx: &mut App| {
        if let Err(e) = cx.text_system().add_fonts(FONTS.iter().map(|&f| Cow::Borrowed(f)).collect()) {
            tracing::warn!(target: "r3manager::ui", error = %e, "couldn't load the built-in fonts");
        }
        bind_keys(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(Bounds::centered(None, size(px(1280.), px(800.)), cx))),
            app_id: Some(APP_ID.to_owned()),
            ..Default::default()
        };
        match cx.open_window(options, |window, cx| {
            window.set_window_title("r3manager");
            cx.new(|cx| Root::new(engine, files, window, cx))
        }) {
            Ok(_) => cx
                .on_window_closed(|cx, _| {
                    if cx.windows().is_empty() {
                        cx.quit();
                    }
                })
                .detach(),
            Err(e) => {
                tracing::error!(target: "r3manager::ui", error = %e, "couldn't open the window");
                cx.quit();
            }
        }
        cx.activate(true);
    });
    Ok(())
}
