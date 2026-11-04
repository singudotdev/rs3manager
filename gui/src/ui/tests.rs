//! The window driven as a developer would, clicks and keys, against a real radosgw: ignored in a plain `cargo test`,
//! run by `dev/test-rgw.sh` with its test account's keys. The core's own tests check every operation's result on the
//! gateway; these check that the window reaches them and shows what comes back.

use super::root::{Modal, Root};
use super::{bind_keys, input::TextInput};
use crate::state::browser::Row;
use gpui::{Entity, Modifiers, TestAppContext, VisualTestContext};
use r3manager_core::profiles::{Files, ProfileEdit};
use r3manager_core::s3::{Engine, Headers, Progress, Session};
use std::path::Path;
use std::time::Duration;

/// A setting the test run needs, from `dev/test-rgw.sh`.
fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} unset: run the tests with dev/test-rgw.sh"))
}

/// Waits for `future` outside GPUI.
fn wait<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(future)
}

/// AWS files in `dir` with profile `test` holding the test account's keys.
fn files(dir: &Path) -> Files {
    let files = Files::in_dir(dir);
    let edit = ProfileEdit::new(
        "test",
        &env("R3M_TEST_ENDPOINT"),
        "",
        &env("R3M_TEST_ACCESS_KEY"),
        &env("R3M_TEST_SECRET_KEY"),
    );
    files.save(None, &edit.unwrap()).unwrap();
    files
}

/// A new bucket holding `docs/readme.txt` and `notes.txt`, made through the core, and its name.
fn bucket(files: &Files, dir: &Path) -> String {
    // Tests run side by side and may start in the same millisecond: the counter keeps their buckets apart.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let engine = Engine::new().unwrap();
    let session: Session = wait(engine.connect(files, "test")).unwrap();
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
    let name = format!("ui-{stamp}-{}", NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed));
    wait(session.create_bucket(&name)).unwrap();
    for (key, text) in [("docs/readme.txt", "hello from the docs"), ("notes.txt", "notes")] {
        let path = dir.join("upload");
        std::fs::write(&path, text).unwrap();
        let headers = Headers { content_type: Some("text/plain".into()), ..Headers::default() };
        wait(session.upload(&name, key, &path, headers, Progress::new())).unwrap();
    }
    name
}

/// The window on the test profile, once its buckets are listed.
fn window(cx: &mut TestAppContext, files: Files) -> (Entity<Root>, &mut VisualTestContext) {
    // Answers come from the core's tokio threads and a real gateway, not the test scheduler's deterministic clock.
    cx.executor().allow_parking();
    cx.update(bind_keys);
    let engine = Engine::new().unwrap();
    let (view, cx) = cx.add_window_view(|window, cx| Root::new(engine, files, window, cx));
    until(cx, &view, "the buckets", |r| r.session.is_some() && !r.buckets_loading);
    (view, cx)
}

/// Runs the window until `done` holds: the gateway answers on the core's own threads, so this polls.
fn until(cx: &mut VisualTestContext, view: &Entity<Root>, what: &str, done: impl Fn(&Root) -> bool) {
    for _ in 0..500 {
        cx.run_until_parked();
        if view.read_with(cx, |root, _| done(root)) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let shown = view.read_with(cx, |root, _| {
        let dialog = match &root.modal {
            Some(Modal::Prompt { error, busy, .. }) => format!("name dialog (busy {busy}, error {error:?})"),
            Some(Modal::Confirm { title, .. }) => format!("confirm {title}"),
            Some(Modal::Profiles(_)) => "profiles".to_owned(),
            Some(Modal::Share(_)) => "share".to_owned(),
            None => "none".to_owned(),
        };
        let rows: Vec<_> = root.browser.rows().map(|r| r.key().to_owned()).collect();
        format!("dialog: {dialog}; message: {:?}; rows: {rows:?}", root.notice.as_ref().map(|n| n.text.clone()))
    });
    panic!("timed out waiting for {what}: {shown}");
}

/// Clicks the element drawn with debug selector `selector`.
fn click(cx: &mut VisualTestContext, selector: &str) {
    let selector: &'static str = Box::leak(selector.to_owned().into_boxed_str());
    let at = cx.debug_bounds(selector).unwrap_or_else(|| panic!("{selector} isn't drawn")).center();
    cx.simulate_click(at, Modifiers::none());
}

/// The keys of the rows shown.
fn rows(view: &Entity<Root>, cx: &mut VisualTestContext) -> Vec<String> {
    view.read_with(cx, |root, _| root.browser.rows().map(|r| r.key().to_owned()).collect())
}

/// Replaces the text of `field`, as typing over it would.
fn type_into(cx: &mut VisualTestContext, field: &Entity<TextInput>, text: &str) {
    field.update(cx, |input, cx| input.set_text(text.to_owned(), cx));
}

#[gpui::test]
#[ignore = "needs a radosgw: dev/test-rgw.sh"]
fn browsing_previewing_and_editing_headers(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let files = files(dir.path());
    let name = bucket(&files, dir.path());
    let (view, cx) = window(cx, files);

    click(cx, &format!("bucket {name}"));
    until(cx, &view, "the bucket's rows", |r| !r.browser.loading() && r.browser.len() == 2);
    assert_eq!(rows(&view, cx), ["docs/", "notes.txt"]);
    // Rows span the list, so their size and date line up under the header's columns.
    let header = cx.debug_bounds("rows header").expect("the header is drawn");
    let row = cx.debug_bounds("row notes.txt").expect("the row is drawn");
    assert_eq!((row.left(), row.size.width), (header.left(), header.size.width));

    click(cx, "row docs/");
    until(cx, &view, "the folder", |r| r.browser.prefix() == "docs/" && !r.browser.loading());
    assert_eq!(rows(&view, cx), ["docs/readme.txt"]);
    cx.simulate_keystrokes("backspace");
    until(cx, &view, "the top again", |r| r.browser.prefix().is_empty() && !r.browser.loading());

    click(cx, "row notes.txt");
    until(cx, &view, "the details", |r| r.details.as_ref().is_some_and(|d| d.form.is_some()));
    until(
        cx,
        &view,
        "the preview",
        |r| matches!(&r.details.as_ref().unwrap().preview, super::browse::PreviewState::Text(t, false) if t.as_ref() == "notes"),
    );
    let content_type =
        view.read_with(cx, |r, _| r.details.as_ref().unwrap().form.as_ref().unwrap().content_type.clone());
    type_into(cx, &content_type, "text/markdown");
    click(cx, "button Save headers");
    until(cx, &view, "the saved headers", |r| {
        r.details.as_ref().and_then(|d| d.info.as_ref()).and_then(|i| i.headers.content_type.as_deref())
            == Some("text/markdown")
    });

    // A share link, signed for a day, then for an hour once picked; enter copies it.
    click(cx, "button Share link");
    let link = |r: &Root| match &r.modal {
        Some(Modal::Share(d)) => d.link.as_ref().map(|(l, _)| (d.expiry, l.to_string())),
        _ => None,
    };
    until(cx, &view, "the day link", |r| link(r).is_some_and(|(e, _)| e == super::share::Expiry::Day));
    assert!(cx.debug_bounds("share link").is_some(), "the link is shown");
    click(cx, "button 1 hour");
    until(cx, &view, "the hour link", |r| link(r).is_some_and(|(e, _)| e == super::share::Expiry::Hour));
    let (_, signed) = view.read_with(cx, |r, _| link(r).unwrap());
    assert!(signed.contains(&format!("/{name}/notes.txt?")) && signed.contains("X-Amz-Expires=3600"), "{signed}");
    cx.simulate_keystrokes("enter");
    assert_eq!(cx.read_from_clipboard().and_then(|c| c.text()), Some(signed));
}

#[gpui::test]
#[ignore = "needs a radosgw: dev/test-rgw.sh"]
fn creating_renaming_and_deleting_from_the_window(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let files = files(dir.path());
    let name = bucket(&files, dir.path());
    let (view, cx) = window(cx, files);
    click(cx, &format!("bucket {name}"));
    until(cx, &view, "the bucket's rows", |r| r.browser.len() == 2);

    // A new folder, through its dialog and the enter key.
    click(cx, "button New folder");
    let input = view.read_with(cx, |r, _| match &r.modal {
        Some(Modal::Prompt { input, .. }) => input.clone(),
        _ => panic!("the name dialog is open"),
    });
    type_into(cx, &input, "reports");
    cx.simulate_keystrokes("enter");
    until(cx, &view, "the new folder", |r| r.modal.is_none() && r.browser.rows().any(|row| row.key() == "reports/"));

    // F2 renames the selected object, its name typed over.
    click(cx, "row notes.txt");
    cx.simulate_keystrokes("f2");
    let input = view.read_with(cx, |r, _| match &r.modal {
        Some(Modal::Prompt { input, .. }) => input.clone(),
        _ => panic!("the rename dialog is open"),
    });
    assert_eq!(input.read_with(cx, |i, _| i.text().to_owned()), "notes.txt");
    type_into(cx, &input, "reports/notes.md");
    cx.simulate_keystrokes("enter");
    until(cx, &view, "the rename", |r| r.modal.is_none() && !r.browser.rows().any(|row| row.key() == "notes.txt"));

    // Delete asks first; enter confirms. The folder goes with what's in it.
    click(cx, "row reports/");
    until(cx, &view, "the reports folder", |r| r.browser.prefix() == "reports/" && !r.browser.loading());
    assert_eq!(rows(&view, cx), ["reports/notes.md"]);
    cx.simulate_keystrokes("backspace");
    until(cx, &view, "the top again", |r| r.browser.prefix().is_empty() && r.browser.len() == 2);
    view.update(cx, |r, cx| {
        r.browser.click("reports/", true, false);
        cx.notify();
    });
    cx.simulate_keystrokes("delete");
    assert!(view.read_with(cx, |r, _| matches!(r.modal, Some(Modal::Confirm { .. }))), "delete asks first");
    cx.simulate_keystrokes("enter");
    until(cx, &view, "the delete", |r| r.browser.len() == 1 && !r.browser.loading());
    assert_eq!(rows(&view, cx), ["docs/"]);

    // Files dropped on the window go up into the open folder.
    let local = dir.path().join("dropped");
    std::fs::create_dir_all(local.join("sub")).unwrap();
    std::fs::write(local.join("sub/a.json"), "{}").unwrap();
    view.update(cx, |r, cx| r.upload_paths(vec![local], cx));
    until(cx, &view, "the upload", |r| !r.transfers.busy() && r.browser.rows().any(|row| row.key() == "dropped/"));
    let row = view.read_with(cx, |r, _| r.transfers.items()[0].status.clone());
    assert_eq!(row, crate::state::transfers::Status::Done);
    assert!(matches!(view.read_with(cx, |r, _| r.browser.row(0).cloned()), Some(Row::Folder(_))));
}

#[gpui::test]
#[ignore = "needs a radosgw: dev/test-rgw.sh"]
fn profiles_are_added_and_switched_in_the_dialog(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let files = files(dir.path());
    let (view, cx) = window(cx, files.clone());

    click(cx, "button test  ▾");
    click(cx, "button Add profile");
    let form = view.read_with(cx, |r, _| match &r.modal {
        Some(Modal::Profiles(super::profiles::ProfilesDialog { form: Some(f) })) => {
            [f.name.clone(), f.endpoint.clone(), f.access_key.clone(), f.secret.clone()]
        }
        _ => panic!("the profile form is open"),
    });
    for (field, text) in
        form.iter().zip(["second", &env("R3M_TEST_ENDPOINT"), &env("R3M_TEST_ACCESS_KEY"), &env("R3M_TEST_SECRET_KEY")])
    {
        type_into(cx, field, text);
    }
    cx.simulate_keystrokes("enter");
    until(cx, &view, "the new profile in use", |r| r.profile.as_deref() == Some("second") && r.session.is_some());
    let names: Vec<_> = files.load().unwrap().into_iter().map(|p| p.name).collect();
    assert_eq!(names, ["test", "second"]);
    let config = std::fs::read_to_string(&files.config).unwrap();
    assert!(config.contains("[profile second]\nendpoint_url = "), "{config}");
}
