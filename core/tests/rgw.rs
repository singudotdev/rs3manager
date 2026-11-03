//! The core against a real radosgw: every operation the app offers, through a profile written by `profiles` and
//! loaded by the SDK, as a developer's would be. Ignored in a plain `cargo test`; `dev/test-rgw.sh` starts the local
//! radosgw and runs them with its test user's keys.

use r3manager_core::profiles::{Files, ProfileEdit};
use r3manager_core::s3::{Engine, Headers, Progress, S3Error, Session};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};

/// Waits for `future` on a throwaway executor, as the app's own (GPUI's) would.
fn wait<T>(future: impl Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(future)
}

/// A setting the test run needs, from `dev/test-rgw.sh`.
fn env(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("{name} unset: run the tests with dev/test-rgw.sh"))
}

/// A profile `test` in its own folder of AWS files, with the test user's keys (or `secret` instead).
fn profile(dir: &Path, secret: Option<&str>) -> Files {
    let files = Files::in_dir(dir);
    let edit = ProfileEdit::new(
        "test",
        &env("R3M_TEST_ENDPOINT"),
        "",
        &env("R3M_TEST_ACCESS_KEY"),
        secret.unwrap_or(&env("R3M_TEST_SECRET_KEY")),
    )
    .unwrap();
    files.save(None, &edit).unwrap();
    files
}

/// The engine, a session as the test profile, and a new empty bucket only this test uses.
fn setup(tag: &str) -> (Engine, Session, String, tempfile::TempDir) {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new().unwrap();
    let session = wait(engine.connect(&profile(dir.path(), None), "test")).unwrap();
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
    let bucket = format!("t-{tag}-{stamp}-{}", NEXT.fetch_add(1, Ordering::Relaxed));
    wait(session.create_bucket(&bucket)).unwrap();
    (engine, session, bucket, dir)
}

/// Uploads `contents` as `key` with `headers`.
fn put(session: &Session, bucket: &str, dir: &Path, key: &str, contents: &[u8], headers: Headers) {
    let path = dir.join("upload");
    std::fs::write(&path, contents).unwrap();
    wait(session.upload(bucket, key, &path, headers, Progress::new())).unwrap();
}

/// Every key at the top of folder `prefix`, folders first.
fn names(session: &Session, bucket: &str, prefix: &str) -> Vec<String> {
    let page = wait(session.list(bucket, prefix, None)).unwrap();
    page.folders.into_iter().chain(page.objects.into_iter().map(|o| o.key)).collect()
}

#[test]
#[ignore = "needs a radosgw: dev/test-rgw.sh"]
fn buckets_are_created_listed_and_deleted() {
    let (_engine, session, bucket, dir) = setup("buckets");
    assert!(wait(session.list_buckets()).unwrap().iter().any(|b| b.name == bucket && b.created.is_some()));
    assert!(matches!(wait(session.create_bucket(&bucket)), Err(S3Error::BucketExists)));
    assert!(matches!(wait(session.create_bucket("Bad_Name")), Err(S3Error::Invalid(_))));

    put(&session, &bucket, dir.path(), "a/b.txt", b"x", Headers::default());
    assert!(matches!(wait(session.delete_bucket(&bucket, false)), Err(S3Error::BucketNotEmpty)));
    wait(session.delete_bucket(&bucket, true)).unwrap();
    assert!(!wait(session.list_buckets()).unwrap().iter().any(|b| b.name == bucket));
}

#[test]
#[ignore = "needs a radosgw: dev/test-rgw.sh"]
fn objects_and_folders_round_trip_with_their_headers() {
    let (_engine, session, bucket, dir) = setup("objects");
    let mut headers = Headers {
        content_type: Some("application/json".into()),
        cache_control: Some("max-age=60".into()),
        ..Headers::default()
    };
    headers.user.insert("team".into(), "platform".into());
    put(&session, &bucket, dir.path(), "docs/readme.json", b"{\"hello\": 1}", headers.clone());
    wait(session.create_folder(&bucket, "empty/")).unwrap();

    assert_eq!(names(&session, &bucket, ""), ["docs/", "empty/"]);
    assert_eq!(names(&session, &bucket, "docs/"), ["docs/readme.json"]);
    // The folder's own marker isn't listed as an object in it.
    assert_eq!(names(&session, &bucket, "empty/"), Vec::<String>::new());

    let info = wait(session.head(&bucket, "docs/readme.json")).unwrap();
    assert_eq!((info.size, &info.headers), (12, &headers));

    let preview = wait(session.preview(&bucket, "docs/readme.json", 5)).unwrap();
    assert_eq!((preview.bytes.as_slice(), preview.truncated), (b"{\"hel".as_slice(), true));

    headers.user.insert("owner".into(), "dev".into());
    headers.content_type = Some("text/plain".into());
    let updated = wait(session.set_headers(&bucket, "docs/readme.json", headers.clone())).unwrap();
    assert_eq!(updated.headers, headers);

    wait(session.rename(&bucket, "docs/readme.json", "docs/renamed.json")).unwrap();
    put(&session, &bucket, dir.path(), "docs/other.txt", b"o", Headers::default());
    let clash = wait(session.rename(&bucket, "docs/other.txt", "docs/renamed.json"));
    assert!(matches!(clash, Err(S3Error::ObjectExists(_))), "{clash:?}");
    // Headers travel with a rename.
    assert_eq!(wait(session.head(&bucket, "docs/renamed.json")).unwrap().headers, headers);

    assert_eq!(wait(session.rename_folder(&bucket, "docs/", "moved/docs/")).unwrap(), 2);
    assert!(matches!(wait(session.rename_folder(&bucket, "moved/", "moved/in/")), Err(S3Error::Invalid(_))));
    assert_eq!(names(&session, &bucket, ""), ["empty/", "moved/"]);

    let dest = dir.path().join("out/renamed.json");
    wait(session.download(&bucket, "moved/docs/renamed.json", &dest, Progress::new())).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b"{\"hello\": 1}");

    let missing = dir.path().join("out/missing");
    assert!(matches!(wait(session.download(&bucket, "nope", &missing, Progress::new())), Err(S3Error::NotFound(_))));
    assert_eq!(std::fs::read_dir(dir.path().join("out")).unwrap().count(), 1, "no partial file left behind");

    let report = wait(session.delete(&bucket, vec!["moved/docs/other.txt".into()])).unwrap();
    assert_eq!((report.deleted, report.failed.len()), (1, 0));
    let report = wait(session.delete_folder(&bucket, "moved/")).unwrap();
    assert_eq!(report.deleted, 1);
    assert_eq!(wait(session.list_recursive(&bucket, "")).unwrap().len(), 1, "only empty/'s marker is left");
    wait(session.delete_bucket(&bucket, true)).unwrap();
}

#[test]
#[ignore = "needs a radosgw: dev/test-rgw.sh"]
fn large_files_go_up_in_parts_and_come_back_whole() {
    let (_engine, session, bucket, dir) = setup("multipart");
    let contents: Vec<u8> = (0..40u32 * 1024 * 1024 / 4).flat_map(u32::to_le_bytes).collect();
    let path = dir.path().join("big.bin");
    std::fs::write(&path, &contents).unwrap();
    let progress = Progress::new();
    wait(session.upload(&bucket, "big.bin", &path, Headers::default(), progress.clone())).unwrap();
    assert_eq!(progress.get(), (contents.len() as u64, contents.len() as u64));
    assert_eq!(wait(session.head(&bucket, "big.bin")).unwrap().size, contents.len() as u64);

    let back = dir.path().join("back.bin");
    wait(session.download(&bucket, "big.bin", &back, Progress::new())).unwrap();
    assert!(std::fs::read(&back).unwrap() == contents, "downloaded bytes differ");

    let cancelled = Progress::new();
    cancelled.cancel();
    let result = wait(session.upload(&bucket, "never.bin", &path, Headers::default(), cancelled));
    assert!(matches!(result, Err(S3Error::Cancelled)), "{result:?}");
    wait(session.delete_bucket(&bucket, true)).unwrap();
}

#[test]
#[ignore = "needs a radosgw: dev/test-rgw.sh"]
fn a_wrong_secret_is_told_apart() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new().unwrap();
    let session = wait(engine.connect(&profile(dir.path(), Some("wrong-secret")), "test")).unwrap();
    let result = wait(session.list_buckets());
    assert!(matches!(result, Err(S3Error::BadCredentials(_))), "{result:?}");
}
