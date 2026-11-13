//! A two-stage lookahead pipeline for the threaded importer (`threaded:
//! yes`), in the spirit of beets' `util/pipeline.py`: jobs go through a
//! first stage (reading files) and a second (looking up candidates), each
//! on its own thread, while the caller, on the main thread, works through
//! the results in submission order. The caller bounds the lookahead by how
//! many jobs it submits before taking results.
//!
//! Dropping the pipeline cancels it: jobs not yet started are dropped, a
//! stage already running finishes its job and the threads exit. The stages
//! must have no side effects beyond their result (they only read), so an
//! abandoned job leaves nothing behind.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};

/// Lets a running stage notice that the pipeline was dropped, to stop
/// between steps of a long job.
///
/// The flag is a one-way "stop" signal: no data is published through it (jobs
/// and results travel over the channels, whose operations establish the
/// happens-before edges), so a relaxed load/store is enough.
#[derive(Clone, Default)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn is_set(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }

    fn set(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

pub struct Lookahead<J, R> {
    jobs: Option<Sender<J>>,
    results: Receiver<R>,
    cancel: Cancel,
    in_flight: usize,
}

impl<J: Send + 'static, R: Send + 'static> Lookahead<J, R> {
    /// Start the two stage threads.
    pub fn new<M: Send + 'static>(
        mut first: impl FnMut(J, &Cancel) -> M + Send + 'static,
        mut second: impl FnMut(M, &Cancel) -> R + Send + 'static,
    ) -> Self {
        let cancel = Cancel::default();
        let (job_tx, job_rx) = mpsc::channel::<J>();
        let (mid_tx, mid_rx) = mpsc::channel::<M>();
        let (res_tx, res_rx) = mpsc::channel::<R>();
        let c = cancel.clone();
        std::thread::spawn(move || {
            for job in job_rx {
                if c.is_set() {
                    return;
                }
                let m = first(job, &c);
                if c.is_set() || mid_tx.send(m).is_err() {
                    return;
                }
            }
        });
        let c = cancel.clone();
        std::thread::spawn(move || {
            for m in mid_rx {
                if c.is_set() {
                    return;
                }
                let r = second(m, &c);
                if c.is_set() || res_tx.send(r).is_err() {
                    return;
                }
            }
        });
        Lookahead {
            jobs: Some(job_tx),
            results: res_rx,
            cancel,
            in_flight: 0,
        }
    }

    /// Queue a job behind the others; false if the pipeline has died.
    pub fn submit(&mut self, job: J) -> bool {
        match &self.jobs {
            Some(tx) if tx.send(job).is_ok() => {
                self.in_flight += 1;
                true
            }
            _ => false,
        }
    }

    /// Jobs submitted whose results haven't been taken.
    pub fn in_flight(&self) -> usize {
        self.in_flight
    }

    /// The oldest job's result, waiting for it if needed. None when
    /// nothing is in flight, or if a stage died (panicked).
    pub fn next(&mut self) -> Option<R> {
        if self.in_flight == 0 {
            return None;
        }
        match self.results.recv() {
            Ok(r) => {
                self.in_flight -= 1;
                Some(r)
            }
            Err(_) => {
                self.in_flight = 0;
                self.jobs = None;
                None
            }
        }
    }
}

impl<J, R> Drop for Lookahead<J, R> {
    fn drop(&mut self) {
        self.cancel.set();
        // Closing the job channel ends the first stage, which ends the
        // second. The threads are not joined: they only read, and a lookup
        // in progress shouldn't hold up an abort.
        self.jobs = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::time::{Duration, Instant};

    #[test]
    fn results_come_back_in_submission_order() {
        // The first stage takes longer for early jobs; order still holds.
        let mut p = Lookahead::new(
            |n: u64, _: &Cancel| {
                std::thread::sleep(Duration::from_millis(10 * (5 - n.min(5))));
                n * 10
            },
            |m: u64, _: &Cancel| m + 1,
        );
        for n in 0..8 {
            p.submit(n);
        }
        assert_eq!(p.in_flight(), 8);
        let got: Vec<u64> = std::iter::from_fn(|| p.next()).collect();
        assert_eq!(got, vec![1, 11, 21, 31, 41, 51, 61, 71]);
        assert_eq!(p.in_flight(), 0);
        assert_eq!(p.next(), None);
    }

    #[test]
    fn dropping_cancels_queued_jobs() {
        let started = Arc::new(Mutex::new(vec![]));
        let s = started.clone();
        let mut p = Lookahead::new(
            |n: u32, _: &Cancel| n,
            move |n: u32, c: &Cancel| {
                s.lock().unwrap().push(n);
                // A slow lookup that checks for cancellation as it goes.
                for _ in 0..50 {
                    if c.is_set() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                n
            },
        );
        for n in 0..10 {
            p.submit(n);
        }
        assert_eq!(p.next(), Some(0));
        let t = Instant::now();
        drop(p);
        // Dropping doesn't wait for the lookup in progress.
        assert!(t.elapsed() < Duration::from_millis(50));
        std::thread::sleep(Duration::from_millis(250));
        let n = started.lock().unwrap().len();
        // Job 0 and perhaps job 1 (running when dropped); nothing after.
        assert!(n <= 2, "{n} lookups started");
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(started.lock().unwrap().len(), n);
    }

    #[test]
    fn a_panicking_stage_ends_the_results() {
        let mut p = Lookahead::new(
            |n: u32, _: &Cancel| n,
            |n: u32, _: &Cancel| {
                assert!(n < 1, "boom");
                n
            },
        );
        p.submit(0);
        p.submit(1);
        p.submit(2);
        assert_eq!(p.next(), Some(0));
        assert_eq!(p.next(), None);
        assert_eq!(p.in_flight(), 0);
        assert!(!p.submit(3));
        assert_eq!(p.in_flight(), 0);
    }

    /// Lookups overlap the caller's work: five 100 ms lookups and five
    /// 100 ms prompts take about 600 ms, not 1 s. Timing-dependent, so
    /// ignored by default (`cargo test -- --ignored`).
    #[test]
    #[ignore]
    fn lookups_overlap_the_callers_work() {
        let step = Duration::from_millis(100);
        let t = Instant::now();
        let mut p = Lookahead::new(
            |n: u32, _: &Cancel| n,
            move |n: u32, _: &Cancel| {
                std::thread::sleep(step);
                n
            },
        );
        let mut next = 0;
        for n in 0..5 {
            while next < 5 && p.in_flight() < 3 {
                p.submit(next);
                next += 1;
            }
            assert_eq!(p.next(), Some(n));
            std::thread::sleep(step); // the user answering the prompt
        }
        let took = t.elapsed();
        eprintln!("5 lookups + 5 prompts: {took:?} (sequential: ~1s)");
        assert!(took < Duration::from_millis(800), "{took:?}");
    }
}
