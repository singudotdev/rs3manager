//! r3manager: a desktop app for developers to manage the buckets and objects of the radosgw account behind each of
//! their AWS CLI profiles. [`state`] is what the window shows, as plain data; [`ui`] draws it with GPUI and runs
//! every operation through the core.

// A desktop app: no console window on Windows; errors go to the log and the window.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod state;
mod ui;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "r3manager=info".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    ui::run()
}
