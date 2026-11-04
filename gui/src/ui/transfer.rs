//! Uploads and downloads from the window: the file pickers, files dropped on it, and the queue that runs a few at a
//! time on the session, redrawing their progress while any runs.

use super::root::{Modal, Root};
use crate::state::browser::Row;
use crate::state::format::{base_name, key_in};
use crate::state::transfers::{Job, Status};
use gpui::{AsyncApp, Context, PathPromptOptions, WeakEntity, Window};
use r3manager_core::s3::names::content_type_for;
use r3manager_core::s3::{Headers, local_path_for, walk};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How often progress is redrawn while transfers run.
const TICK: Duration = Duration::from_millis(250);

/// Where downloads go by default: `~/Downloads` when it exists, else home.
fn downloads_dir() -> PathBuf {
    let home = std::env::home_dir().unwrap_or_else(|| PathBuf::from("."));
    let downloads = home.join("Downloads");
    if downloads.is_dir() { downloads } else { home }
}

/// An upload of `path` to `key`, its Content-Type guessed from its name.
fn upload(bucket: &str, key: String, path: PathBuf) -> Job {
    let headers = Headers { content_type: Some(content_type_for(&key).to_owned()), ..Headers::default() };
    Job::Upload { bucket: bucket.to_owned(), key, path, headers }
}

/// The uploads of `paths` into folder `prefix` of `bucket`: a file by its name, a folder by its name and every file
/// under it. Runs on a background thread, as walking a large folder takes a while.
fn uploads_for(bucket: &str, prefix: &str, paths: &[PathBuf]) -> (Vec<Job>, Vec<String>) {
    let mut jobs = Vec::new();
    let mut errors = Vec::new();
    for path in paths {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        if path.is_dir() {
            match walk(path) {
                Ok(files) => {
                    jobs.extend(
                        files
                            .into_iter()
                            .map(|(file, rel)| upload(bucket, key_in(prefix, &format!("{name}/{rel}")), file)),
                    );
                }
                Err(e) => errors.push(format!("{}: {e}", path.display())),
            }
        } else if path.is_file() {
            jobs.push(upload(bucket, key_in(prefix, &name), path.clone()));
        }
    }
    (jobs, errors)
}

impl Root {
    /// Picks files (or, with `folders`, folders) to upload into the open folder.
    pub fn pick_uploads(&mut self, folders: bool, cx: &mut Context<Self>) {
        if self.browser.bucket().is_none() || !self.may_write_here() {
            return;
        }
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: !folders,
            directories: folders,
            multiple: true,
            prompt: Some(if folders { "Upload folders" } else { "Upload files" }.into()),
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let paths = match picked.await {
                Ok(Ok(Some(paths))) => paths,
                Ok(Ok(None)) | Err(_) => return,
                Ok(Err(e)) => {
                    let _ = this.update(cx, |this, cx| this.error(format!("Couldn't open the file picker: {e}"), cx));
                    return;
                }
            };
            let _ = this.update(cx, |this, cx| this.upload_paths(paths, cx));
        })
        .detach();
    }

    /// Uploads `paths` (files or folders, picked or dropped) into the open folder.
    pub fn upload_paths(&mut self, paths: Vec<PathBuf>, cx: &mut Context<Self>) {
        let Some(bucket) = self.browser.bucket().map(str::to_owned) else {
            self.error("Open a bucket first, then drop the files on it", cx);
            return;
        };
        if !self.may_write_here() {
            self.error(format!("This profile's policies don't allow uploading into {bucket}"), cx);
            return;
        }
        let prefix = self.browser.prefix().to_owned();
        let walking = cx.background_executor().spawn(async move { uploads_for(&bucket, &prefix, &paths) });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let (jobs, errors) = walking.await;
            let _ = this.update(cx, |this, cx| {
                if let Some(e) = errors.first() {
                    this.error(format!("Couldn't read {e}"), cx);
                }
                this.ask_before_replacing(jobs, cx);
            });
        })
        .detach();
    }

    /// Queues `jobs`, uploads into one bucket, once the developer has chosen what to do with those that would
    /// replace an object: S3 overwrites without a word, and a lost object is gone unless the bucket keeps versions.
    fn ask_before_replacing(&mut self, jobs: Vec<Job>, cx: &mut Context<Self>) {
        let Some(Job::Upload { bucket, .. }) = jobs.first() else { return };
        let bucket = bucket.clone();
        let keys = jobs.iter().filter_map(|j| if let Job::Upload { key, .. } = j { Some(key.clone()) } else { None });
        let keys = keys.collect();
        self.run(
            cx,
            move |s| s.existing(&bucket, keys),
            |this, result, cx| match result {
                Ok(existing) if existing.is_empty() => this.queue(jobs, cx),
                Ok(existing) => this.modal = Some(Modal::Replace { jobs, existing }),
                Err(e) => this.error(format!("Couldn't check what the upload would replace: {e}"), cx),
            },
        );
    }

    /// Runs the uploads of the replace dialog: all of them when `replace`, else only those of new keys.
    pub fn replace_chosen(&mut self, replace: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(Modal::Replace { jobs, existing }) = self.modal.take() else { return };
        self.close_modal(window, cx);
        if replace {
            self.queue(jobs, cx);
            return;
        }
        let fresh: Vec<Job> =
            jobs.into_iter().filter(|j| !matches!(j, Job::Upload { key, .. } if existing.contains(key))).collect();
        if fresh.is_empty() {
            self.info("Nothing uploaded: every file is already there", cx);
        } else {
            self.info(format!("Skipped {} file(s) already there", existing.len()), cx);
            self.queue(fresh, cx);
        }
    }

    /// Downloads the selected rows: one object to a file picked by name; anything else into a folder picked, keeping
    /// the folders' structure below the open folder.
    pub fn download_selected(&mut self, cx: &mut Context<Self>) {
        let Some(bucket) = self.browser.bucket().map(str::to_owned) else { return };
        let rows: Vec<Row> = self.browser.selection().into_iter().cloned().collect();
        match rows.as_slice() {
            [] => {}
            [Row::Object(object)] => {
                let key = object.key.clone();
                let picked = cx.prompt_for_new_path(&downloads_dir(), Some(base_name(&key)));
                cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                    if let Ok(Ok(Some(dest))) = picked.await {
                        let _ = this.update(cx, |this, cx| this.queue(vec![Job::Download { bucket, key, dest }], cx));
                    }
                })
                .detach();
            }
            _ => self.download_into_folder(bucket, rows, cx),
        }
    }

    /// Asks for a folder, then downloads `rows` into it.
    fn download_into_folder(&mut self, bucket: String, rows: Vec<Row>, cx: &mut Context<Self>) {
        let prefix = self.browser.prefix().to_owned();
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Download here".into()),
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let Ok(Ok(Some(mut dirs))) = picked.await else { return };
            let Some(target) = dirs.pop() else { return };
            let _ = this.update(cx, |this, cx| {
                for row in rows {
                    match row {
                        Row::Object(object) => this.queue_download(&bucket, &prefix, &target, object.key, cx),
                        Row::Folder(folder) => this.download_folder(&bucket, &prefix, &target, &folder, cx),
                    }
                }
            });
        })
        .detach();
    }

    /// Queues the download of every object under `folder` into `target`.
    fn download_folder(&mut self, bucket: &str, prefix: &str, target: &Path, folder: &str, cx: &mut Context<Self>) {
        let (bucket, prefix, target) = (bucket.to_owned(), prefix.to_owned(), target.to_owned());
        let b = bucket.clone();
        self.run(
            cx,
            move |s| s.list_recursive(&b, folder),
            move |this, result, cx| match result {
                Ok(objects) => {
                    for object in objects.into_iter().filter(|o| !o.key.ends_with('/')) {
                        this.queue_download(&bucket, &prefix, &target, object.key, cx);
                    }
                }
                Err(e) => this.error(format!("Couldn't list what to download: {e}"), cx),
            },
        );
    }

    /// Queues the download of `key` to its path below `target`, relative to the open folder `prefix`; a key that
    /// would land outside `target` is refused.
    fn queue_download(&mut self, bucket: &str, prefix: &str, target: &Path, key: String, cx: &mut Context<Self>) {
        let relative = key.strip_prefix(prefix).unwrap_or(&key);
        match local_path_for(target, relative) {
            Some(dest) => self.queue(vec![Job::Download { bucket: bucket.to_owned(), key, dest }], cx),
            None => self.error(format!("Skipped {key}: its name isn't a safe path to save to"), cx),
        }
    }

    /// Adds `jobs` to the queue and starts what can run.
    pub fn queue(&mut self, jobs: Vec<Job>, cx: &mut Context<Self>) {
        if jobs.is_empty() {
            return;
        }
        for job in jobs {
            self.transfers.add(job);
        }
        self.transfers_open = true;
        self.start_transfers(cx);
        self.tick(cx);
        cx.notify();
    }

    /// Starts the queued transfers there's room for.
    fn start_transfers(&mut self, cx: &mut Context<Self>) {
        for (id, job, progress) in self.transfers.start_ready() {
            let job_for_run = job.clone();
            self.run(
                cx,
                move |s| match job_for_run {
                    Job::Upload { bucket, key, path, headers } => s.upload(&bucket, &key, &path, headers, progress),
                    Job::Download { bucket, key, dest } => s.download(&bucket, &key, &dest, progress),
                },
                move |this, result, cx| this.transfer_finished(id, result, cx),
            );
        }
    }

    /// Records how transfer `id` ended, starts the next ones, and lists the open folder again once the last upload
    /// into it is done.
    fn transfer_finished(&mut self, id: u64, result: Result<(), r3manager_core::s3::S3Error>, cx: &mut Context<Self>) {
        let uploaded_here = match self.transfers.finish(id, result) {
            Some(Job::Upload { bucket, .. }) => self.browser.bucket() == Some(bucket.as_str()),
            _ => false,
        };
        self.start_transfers(cx);
        let more_uploads_here = self.transfers.items().iter().any(|t| {
            matches!(t.status, Status::Queued | Status::Running)
                && matches!(&t.job, Job::Upload { bucket, .. } if self.browser.bucket() == Some(bucket.as_str()))
        });
        if uploaded_here && !more_uploads_here {
            self.reload_folder(cx);
        }
    }

    /// Redraws every [`TICK`] while transfers run, so their progress moves.
    fn tick(&mut self, cx: &mut Context<Self>) {
        if self.ticking {
            return;
        }
        self.ticking = true;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            loop {
                cx.background_executor().timer(TICK).await;
                let busy = this.update(cx, |this, cx| {
                    cx.notify();
                    this.ticking = this.transfers.busy();
                    this.ticking
                });
                if !matches!(busy, Ok(true)) {
                    break;
                }
            }
        })
        .detach();
    }
}
