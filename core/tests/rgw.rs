//! The core against a real radosgw: every operation the app offers, through a profile written by `profiles` and
//! loaded by the SDK, as a developer's would be. Ignored in a plain `cargo test`; `dev/test-rgw.sh` starts the local
//! radosgw and runs them with its test user's keys.

use r3manager_core::profiles::{Files, ProfileEdit};
use r3manager_core::s3::{Engine, Headers, Progress, S3Error, Session};
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

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
    assert!(wait(session.list_buckets(false)).unwrap().iter().any(|b| b.name == bucket && b.created.is_some()));
    assert!(matches!(wait(session.create_bucket(&bucket)), Err(S3Error::BucketExists)));
    assert!(matches!(wait(session.create_bucket("Bad_Name")), Err(S3Error::Invalid(_))));

    put(&session, &bucket, dir.path(), "a/b.txt", b"x", Headers::default());
    assert!(matches!(wait(session.delete_bucket(&bucket, false)), Err(S3Error::BucketNotEmpty)));
    wait(session.delete_bucket(&bucket, true)).unwrap();
    assert!(!wait(session.list_buckets(false)).unwrap().iter().any(|b| b.name == bucket));
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
    let result = wait(session.list_buckets(false));
    assert!(matches!(result, Err(S3Error::BadCredentials(_))), "{result:?}");
}

/// Status and body of a plain HTTP GET of `url`, with no credentials, as a browser holding a shared link would send.
/// The local gateway speaks plain HTTP, so std's socket is enough.
fn fetch(url: &str) -> (u16, Vec<u8>) {
    use std::io::{Read as _, Write as _};
    let rest = url.strip_prefix("http://").expect("the local gateway is plain HTTP");
    let (host, path) = rest.split_once('/').map_or((rest, "/".to_owned()), |(h, p)| (h, format!("/{p}")));
    let mut stream = std::net::TcpStream::connect(host).unwrap();
    write!(stream, "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").unwrap();
    let mut response = Vec::new();
    stream.read_to_end(&mut response).unwrap();
    let split = response.windows(4).position(|w| w == b"\r\n\r\n").expect("a full response");
    let status = std::str::from_utf8(&response[9..12]).unwrap().parse().unwrap();
    (status, response[split + 4..].to_vec())
}

#[test]
#[ignore = "needs a radosgw: dev/test-rgw.sh"]
fn shared_links_download_without_credentials_until_they_expire() {
    let (_engine, session, bucket, dir) = setup("share");
    // A key that needs escaping in the URL, as a signature over the wrong encoding is refused.
    let key = "docs/año 2026+.txt";
    put(&session, &bucket, dir.path(), key, b"shared", Headers::default());

    let link = wait(session.share(&bucket, key, Duration::from_secs(3600))).unwrap();
    assert_eq!(fetch(&link), (200, b"shared".to_vec()));

    let forged = link.replacen("X-Amz-Expires=3600", "X-Amz-Expires=3601", 1);
    assert_ne!(forged, link);
    assert_eq!(fetch(&forged).0, 403, "a changed link is refused");

    let short = wait(session.share(&bucket, key, Duration::from_secs(1))).unwrap();
    std::thread::sleep(Duration::from_secs(2));
    // S3 answers 403; radosgw 19.2.0 answers 500 UnknownError. Either way it's refused, without the object.
    let (status, body) = fetch(&short);
    assert!(matches!(status, 403 | 500) && !body.ends_with(b"shared"), "an expired link is refused: {status}");

    let week_and_a_bit = Duration::from_secs(7 * 24 * 3600 + 1);
    assert!(matches!(wait(session.share(&bucket, key, week_and_a_bit)), Err(S3Error::Invalid(_))));
    wait(session.delete_bucket(&bucket, true)).unwrap();
}

#[test]
#[ignore = "needs a radosgw: dev/test-rgw.sh"]
fn uploads_learn_which_keys_they_would_replace() {
    let (_engine, session, bucket, dir) = setup("existing");
    for key in ["top.txt", "a/b.txt", "a/deep/c.txt"] {
        put(&session, &bucket, dir.path(), key, b"x", Headers::default());
    }
    let keys = ["top.txt", "new.txt", "a/b.txt", "a/new.txt", "a/deep/c.txt", "fresh/deep/x.txt", "a/deeper/y"];
    let existing = wait(session.existing(&bucket, keys.iter().map(|k| (*k).to_owned()).collect())).unwrap();
    assert_eq!(existing, ["top.txt", "a/b.txt", "a/deep/c.txt"]);
    wait(session.delete_bucket(&bucket, true)).unwrap();
}

/// A session as the identity whose keys `dev/test-rgw.sh` exports under `prefix`.
fn session_as(engine: &Engine, dir: &Path, prefix: &str) -> Session {
    let files = Files::in_dir(dir);
    let (key, secret) = (env(&format!("{prefix}_ACCESS_KEY")), env(&format!("{prefix}_SECRET_KEY")));
    files.save(None, &ProfileEdit::new("p", &env("R3M_TEST_ENDPOINT"), "", &key, &secret).unwrap()).unwrap();
    wait(engine.connect(&files, "p")).unwrap()
}

#[test]
#[ignore = "needs a radosgw: dev/test-rgw.sh"]
fn group_policies_decide_what_a_project_user_may_do() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new().unwrap();
    let (root, ro, rw) = (
        session_as(&engine, &dir.path().join("root"), "R3M_TEST_PROD_ROOT"),
        session_as(&engine, &dir.path().join("ro"), "R3M_TEST_PROD_RO"),
        session_as(&engine, &dir.path().join("rw"), "R3M_TEST_DEV_RW"),
    );
    let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis();
    let (ours, theirs) = (format!("r3mtest-{stamp}-prod"), format!("legacy-{stamp}-prod"));
    for bucket in [&ours, &theirs] {
        wait(root.create_bucket(bucket)).unwrap();
        put(&root, bucket, dir.path(), "f.txt", b"x", Headers::default());
    }

    // Production, read-only buckets: every bucket of the account is listed, those out of reach marked.
    let buckets = wait(ro.list_buckets(true)).unwrap();
    let listable = |name: &str| buckets.iter().find(|b| b.name == name).map(|b| b.listable);
    assert_eq!((listable(&ours), listable(&theirs)), (Some(true), Some(false)));
    let denied = |r: Result<(), S3Error>| matches!(r, Err(S3Error::AccessDenied(_)));
    assert!(denied(wait(ro.create_bucket(&format!("r3mtest-new-{stamp}-prod")))), "no new buckets in prod");
    assert!(denied(wait(ro.delete_bucket(&ours, false))));
    put(&ro, &ours, dir.path(), "mine.txt", b"objects are read-write", Headers::default());
    assert_eq!(wait(ro.delete(&ours, vec!["mine.txt".into()])).unwrap().deleted, 1);
    assert!(matches!(wait(ro.list(&theirs, "", None)), Err(S3Error::AccessDenied(_))));

    // Development, read-write buckets, but only under the project's names: a refused name is a refusal, not "taken";
    // a bucket of the account out of the user's reach is taken.
    let mine = format!("r3mtest-{stamp}-dev");
    wait(rw.create_bucket(&mine)).unwrap();
    assert!(denied(wait(rw.create_bucket(&format!("elsewhere-{stamp}-dev")))), "names outside r3mtest-*-dev");
    assert!(matches!(wait(rw.create_bucket(&theirs)), Err(S3Error::BucketExists)), "taken in another account");
    wait(rw.delete_bucket(&mine, false)).unwrap();

    for bucket in [&ours, &theirs] {
        wait(root.delete_bucket(bucket, true)).unwrap();
    }
}

#[test]
#[ignore = "needs a radosgw: dev/test-rgw.sh"]
fn a_project_user_reads_its_own_policies() {
    use r3manager_core::s3::Access;
    let dir = tempfile::tempdir().unwrap();
    let engine = Engine::new().unwrap();
    let ro = wait(session_as(&engine, &dir.path().join("ro"), "R3M_TEST_PROD_RO").access()).unwrap();
    let Access::Policies { user, groups, .. } = &ro else { panic!("policies read: {ro:?}") };
    assert_eq!((user.as_str(), groups.len()), ("tester", 2), "{groups:?}");
    assert!(ro.may("s3:PutObject", "r3mtest-web-prod", Some("a/b.txt")));
    assert!(ro.may("s3:ListBucket", "r3mtest-web-prod", None));
    assert!(!ro.may("s3:ListBucket", "legacy-prod", None));
    assert_eq!(ro.bucket_patterns(), Some(vec![]), "no new buckets in prod");

    let rw = wait(session_as(&engine, &dir.path().join("rw"), "R3M_TEST_DEV_RW").access()).unwrap();
    assert_eq!(rw.bucket_patterns(), Some(vec!["r3mtest-*-dev".to_owned()]));
    assert!(rw.may("s3:DeleteBucket", "r3mtest-x-dev", None) && !rw.may("s3:DeleteBucket", "r3mtest-x-prod", None));

    let root = wait(session_as(&engine, &dir.path().join("root"), "R3M_TEST_PROD_ROOT").access()).unwrap();
    assert!(matches!(root, Access::Root { .. }), "{root:?}");
    // Everything in the account, but the guardrails `dev/rgw.sh account` puts on it.
    assert!(root.may("s3:CreateBucket", "anything-at-all", None) && root.may("s3:DeleteObject", "b", Some("k")));
    assert!(!root.may("s3:PutBucketPolicy", "r3mtest-web-prod", None), "no bucket policies, even for the root");
    assert_eq!(root.bucket_patterns(), None, "any name may be tried");
}
