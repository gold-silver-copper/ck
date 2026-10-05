//! A thread for slow file work (converting, encoding and writing saved copies), so the UI
//! thread only hands it over. Jobs run one at a time, in the order given; what each one
//! reports is collected on the UI thread with `results`.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use crate::http::lock;

type Job<T> = Box<dyn FnOnce() -> T + Send>;

pub struct Writer<T> {
    tx: Sender<Job<T>>,
    done: Receiver<T>,
    /// Jobs given and not yet finished.
    pending: Arc<(Mutex<usize>, Condvar)>,
}

impl<T: Send + 'static> Writer<T> {
    /// `panicked` turns a job's panic into what it reports instead.
    pub fn new(panicked: fn(String) -> T) -> Self {
        let (tx, jobs) = channel::<Job<T>>();
        let (report, done) = channel();
        let pending = Arc::new((Mutex::new(0), Condvar::new()));
        let left = pending.clone();
        std::thread::spawn(move || {
            while let Ok(job) = jobs.recv() {
                let result = crate::guard::catching(job).unwrap_or_else(panicked);
                let gone = report.send(result).is_err();
                *lock(&left.0) -= 1;
                left.1.notify_all();
                if gone {
                    return;
                }
            }
        });
        Writer { tx, done, pending }
    }

    /// Run `job` after the ones before it.
    pub fn run(&self, job: impl FnOnce() -> T + Send + 'static) {
        *lock(&self.pending.0) += 1;
        if self.tx.send(Box::new(job)).is_err() {
            *lock(&self.pending.0) -= 1;
        }
    }

    /// What finished jobs reported, oldest first.
    pub fn results(&self) -> Vec<T> {
        self.done.try_iter().collect()
    }

    /// Whether every job given has finished.
    pub fn is_idle(&self) -> bool {
        *lock(&self.pending.0) == 0
    }

    /// Wait (at most `within`) until every job given so far has finished. False if some
    /// haven't.
    pub fn flush(&self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        let mut left = lock(&self.pending.0);
        while *left > 0 {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            left = self.pending.1.wait_timeout(left, deadline - now).unwrap_or_else(std::sync::PoisonError::into_inner).0;
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_run_in_order_and_flush_waits_for_them() {
        let w = Writer::new(|_| -1);
        for i in 0..50 {
            w.run(move || {
                if i == 0 {
                    std::thread::sleep(Duration::from_millis(30));
                }
                i
            });
        }
        assert!(w.flush(Duration::from_secs(10)));
        assert_eq!(w.results(), (0..50).collect::<Vec<_>>());
        // Nothing pending: flush returns at once.
        let start = Instant::now();
        assert!(w.flush(Duration::from_secs(10)) && start.elapsed() < Duration::from_millis(50));
        // A job that takes too long: flush gives up.
        w.run(|| {
            std::thread::sleep(Duration::from_millis(300));
            99
        });
        assert!(!w.flush(Duration::from_millis(20)));
        assert!(w.flush(Duration::from_secs(10)));
        assert_eq!(w.results(), [99]);
        // A job that panics reports what `panicked` makes of it, and counts as finished.
        w.run(|| std::panic::panic_any("deliberate"));
        w.run(|| 100);
        assert!(w.flush(Duration::from_secs(10)));
        assert_eq!(w.results(), [-1, 100]);
    }
}
