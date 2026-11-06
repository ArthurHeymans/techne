//! A single bounded worker for GPU completion and bulk PNG encoding. The
//! window thread submits a copy and keeps serving input, redraw and close.
//! Scheme still owns capture/recording policy and all file destinations.

use std::sync::mpsc::{self, SyncSender};
use techne_editor::present::CapturedFrame;

pub struct Job {
    pub epoch: u64,
    pub ids: Vec<usize>,
    pub finish: Box<dyn FnOnce() -> Result<CapturedFrame, String> + Send>,
}

#[derive(Debug)]
pub struct Completion {
    pub epoch: u64,
    pub ids: Vec<usize>,
    pub result: Result<CapturedFrame, String>,
}

pub struct Worker(SyncSender<Job>);

impl Worker {
    pub fn new(deliver: impl Fn(Completion) + Send + 'static) -> Self {
        let (tx, rx) = mpsc::sync_channel::<Job>(1);
        std::thread::Builder::new()
            .name("capture".into())
            .spawn(move || {
                for job in rx {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job.finish))
                        .unwrap_or_else(|_| Err("capture worker panicked".into()));
                    deliver(Completion { epoch: job.epoch, ids: job.ids, result });
                }
            })
            .expect("a capture worker");
        Self(tx)
    }

    /// Never wait for a previous capture. App also admits only one batch at
    /// a time; the bounded channel is a second guard against queued pixels.
    pub fn submit(&self, job: Job) -> Result<(), Job> {
        self.0.try_send(job).map_err(|e| match e {
            mpsc::TrySendError::Full(job) | mpsc::TrySendError::Disconnected(job) => job,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_stalled_capture_does_not_block_submission_or_grow_the_queue() {
        let (tx, completions) = mpsc::channel();
        let worker = Worker::new(move |c| {
            let _ = tx.send(c);
        });
        let (started, start) = mpsc::channel();
        let (release, wait) = mpsc::channel();
        assert!(
            worker
                .submit(Job {
                    epoch: 7,
                    ids: vec![12],
                    finish: Box::new(move || {
                        started.send(()).unwrap();
                        wait.recv().unwrap();
                        Err("simulated GPU failure".into())
                    })
                })
                .is_ok()
        );
        start.recv_timeout(Duration::from_secs(2)).unwrap();
        // Even with the first job blocked, enqueueing never waits. A third
        // job is refused instead of retaining another frame's GPU resources.
        let job = || Job { epoch: 8, ids: vec![13], finish: Box::new(|| Err("second".into())) };
        assert!(worker.submit(job()).is_ok());
        assert!(worker.submit(job()).is_err());
        assert!(completions.try_recv().is_err());
        release.send(()).unwrap();
        let c = completions.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!((c.epoch, c.ids), (7, vec![12]));
        assert_eq!(c.result.unwrap_err(), "simulated GPU failure");
        assert_eq!(completions.recv_timeout(Duration::from_secs(2)).unwrap().epoch, 8);
    }

    #[test]
    fn a_worker_panic_completes_the_request_and_the_worker_survives() {
        let (tx, completions) = mpsc::channel();
        let worker = Worker::new(move |c| {
            let _ = tx.send(c);
        });
        assert!(worker.submit(Job { epoch: 0, ids: vec![1], finish: Box::new(|| panic!("test")) }).is_ok());
        assert!(completions.recv_timeout(Duration::from_secs(2)).unwrap().result.unwrap_err().contains("panicked"));
        assert!(worker.submit(Job { epoch: 0, ids: vec![2], finish: Box::new(|| Err("still alive".into())) }).is_ok());
        assert_eq!(completions.recv_timeout(Duration::from_secs(2)).unwrap().result.unwrap_err(), "still alive");
    }
}
