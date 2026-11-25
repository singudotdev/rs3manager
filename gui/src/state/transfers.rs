//! Uploads and downloads the user started: queued, a few running at once, then done, failed or cancelled. Only the
//! bookkeeping lives here; the app runs each job on the session and reports back with [`Transfers::finish`].

use rs3manager_core::s3::{Headers, Progress, S3Error};
use std::path::PathBuf;
use std::sync::Arc;

/// Transfers running at once: enough to keep the link busy without one big folder starving the rest.
pub const RUNNING_AT_ONCE: usize = 3;

/// What a transfer does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Job {
    /// Upload file `path` as object `key` of `bucket`.
    Upload {
        /// The bucket.
        bucket: String,
        /// The object's key.
        key: String,
        /// The local file.
        path: PathBuf,
        /// The headers it goes up with.
        headers: Headers,
    },
    /// Download object `key` of `bucket` to `dest`.
    Download {
        /// The bucket.
        bucket: String,
        /// The object's key.
        key: String,
        /// The local file.
        dest: PathBuf,
    },
}

/// Where a transfer is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Status {
    /// Waiting for a free slot.
    Queued,
    /// Under way.
    Running,
    /// Finished.
    Done,
    /// Stopped by an error, with what it said.
    Failed(String),
    /// Stopped by the user.
    Cancelled,
}

/// One transfer.
#[derive(Debug)]
pub struct Transfer {
    /// Its id, unique in this run.
    pub id: u64,
    /// What it does.
    pub job: Job,
    /// How far it is.
    pub progress: Arc<Progress>,
    /// Where it is.
    pub status: Status,
}

/// Every transfer of this run, oldest first.
#[derive(Debug, Default)]
pub struct Transfers {
    /// The transfers.
    items: Vec<Transfer>,
    /// The next id.
    next_id: u64,
}

impl Transfers {
    /// Queues `job`.
    pub fn add(&mut self, job: Job) {
        self.next_id += 1;
        self.items.push(Transfer { id: self.next_id, job, progress: Progress::new(), status: Status::Queued });
    }

    /// The transfers, oldest first.
    pub fn items(&self) -> &[Transfer] {
        &self.items
    }

    /// Marks queued transfers as running while fewer than [`RUNNING_AT_ONCE`] are, returning the ones to start.
    pub fn start_ready(&mut self) -> Vec<(u64, Job, Arc<Progress>)> {
        let mut free = RUNNING_AT_ONCE.saturating_sub(self.running());
        let mut start = Vec::new();
        for t in self.items.iter_mut().filter(|t| t.status == Status::Queued) {
            if free == 0 {
                break;
            }
            t.status = Status::Running;
            start.push((t.id, t.job.clone(), Arc::clone(&t.progress)));
            free -= 1;
        }
        start
    }

    /// How many are running.
    pub fn running(&self) -> usize {
        self.items.iter().filter(|t| t.status == Status::Running).count()
    }

    /// Whether any is queued or running.
    pub fn busy(&self) -> bool {
        self.items.iter().any(|t| matches!(t.status, Status::Queued | Status::Running))
    }

    /// Records how transfer `id` ended, returning its job.
    pub fn finish(&mut self, id: u64, result: Result<(), S3Error>) -> Option<&Job> {
        let t = self.items.iter_mut().find(|t| t.id == id)?;
        t.status = match result {
            Ok(()) => Status::Done,
            Err(S3Error::Cancelled) => Status::Cancelled,
            Err(e) => Status::Failed(e.to_string()),
        };
        Some(&t.job)
    }

    /// Cancels transfer `id`: a queued one at once, a running one at its next chunk.
    pub fn cancel(&mut self, id: u64) {
        if let Some(t) = self.items.iter_mut().find(|t| t.id == id) {
            t.progress.cancel();
            if t.status == Status::Queued {
                t.status = Status::Cancelled;
            }
        }
    }

    /// Cancels every queued or running transfer.
    pub fn cancel_all(&mut self) {
        let ids: Vec<u64> = self.items.iter().map(|t| t.id).collect();
        for id in ids {
            self.cancel(id);
        }
    }

    /// Forgets the finished ones.
    pub fn clear_finished(&mut self) {
        self.items.retain(|t| matches!(t.status, Status::Queued | Status::Running));
    }

    /// Bytes moved and to move over every unfinished transfer, and how many there are.
    pub fn totals(&self) -> (u64, u64, usize) {
        self.items.iter().filter(|t| matches!(t.status, Status::Queued | Status::Running)).fold(
            (0, 0, 0),
            |(done, total, n), t| {
                let (d, s) = t.progress.get();
                (done + d, total + s, n + 1)
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A download of `key`.
    fn download(key: &str) -> Job {
        Job::Download { bucket: "b".into(), key: key.into(), dest: PathBuf::from(key) }
    }

    #[test]
    fn a_few_run_at_once_and_the_rest_wait() {
        let mut t = Transfers::default();
        for k in ["1", "2", "3", "4", "5"] {
            t.add(download(k));
        }
        let started = t.start_ready();
        assert_eq!(started.iter().map(|s| s.0).collect::<Vec<_>>(), [1, 2, 3]);
        assert!(t.start_ready().is_empty());
        t.finish(2, Ok(()));
        assert_eq!(t.start_ready().iter().map(|s| s.0).collect::<Vec<_>>(), [4]);
        t.finish(1, Err(S3Error::NotFound("x".into())));
        t.finish(3, Err(S3Error::Cancelled));
        let statuses: Vec<_> = t.items().iter().map(|i| i.status.clone()).collect();
        assert_eq!(statuses[0], Status::Failed("not found: x".into()));
        assert_eq!(statuses[2], Status::Cancelled);
    }

    #[test]
    fn cancelling_a_queued_one_never_starts_it() {
        let mut t = Transfers::default();
        for k in ["1", "2", "3", "4"] {
            t.add(download(k));
        }
        t.start_ready();
        t.cancel(4);
        t.finish(1, Ok(()));
        assert!(t.start_ready().is_empty());
        t.clear_finished();
        assert_eq!(t.items().iter().map(|i| i.id).collect::<Vec<_>>(), [2, 3]);
        assert!(t.busy());
    }
}
