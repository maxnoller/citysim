//! Checking plans off the frame. A plan of a large road (a street grid of
//! a hundred blocks, a long chain of curves) is many commands, each one
//! applied to a copy of the city ([`plan::evaluate`]); that can take far
//! longer than a frame. A worker thread checks the newest plan the tools
//! ask for while the game goes on drawing the last one checked: the frame
//! never waits for more than [`WAIT`] (enough for an ordinary road to be
//! checked within its own frame).
//!
//! Each request is a base world (the tools' scratch copy, shared, with an
//! id that changes whenever the copy is made again) and the plan's
//! operations; results come back with the world after the plan. The worker
//! keeps the worlds after each operation of the plan before
//! ([`plan::Prefixes`]), so a plan that only changes at its end (the live
//! piece of a chain) is checked from there. Requests that come while one is
//! being checked replace each other: only the newest is checked next.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use citysim_sim::World;

use super::plan::{self, Evaluated, Op};

/// Longest the frame waits for a plan it has just asked for.
pub const WAIT: Duration = Duration::from_micros(2_500);

/// A checked plan.
pub struct Done {
    pub base_id: u64,
    pub ops: Vec<Op>,
    pub eval: Evaluated,
    /// The base world it was checked on and the world after it.
    pub base: Arc<World>,
    pub after: World,
    /// Time each step took to check, ms (parallel to `eval.steps`).
    pub step_ms: Vec<f32>,
    /// Time the whole check took, ms.
    pub ms: f32,
}

struct Job {
    base_id: u64,
    base: Arc<World>,
    ops: Vec<Op>,
}

/// The worker and what it was asked last.
pub struct Planner {
    tx: Option<Sender<Job>>,
    rx: Option<Mutex<Receiver<Done>>>,
    /// The job being checked (its base id and operations).
    busy: Option<(u64, Vec<Op>)>,
    /// The newest request not yet handed to the worker.
    next: Option<Job>,
    /// The newest result.
    pub last: Option<Done>,
}

impl Default for Planner {
    fn default() -> Self {
        let (tx, jobs) = channel::<Job>();
        let (done_tx, rx) = channel::<Done>();
        let spawned = std::thread::Builder::new()
            .name("tool planner".into())
            .spawn(move || worker(jobs, done_tx));
        match spawned {
            Ok(_) => Planner {
                tx: Some(tx),
                rx: Some(Mutex::new(rx)),
                busy: None,
                next: None,
                last: None,
            },
            // No thread: plans are checked in the frame.
            Err(_) => Planner {
                tx: None,
                rx: None,
                busy: None,
                next: None,
                last: None,
            },
        }
    }
}

fn worker(jobs: Receiver<Job>, out: Sender<Done>) {
    let mut prefixes = plan::Prefixes::default();
    while let Ok(job) = jobs.recv() {
        let t = Instant::now();
        let (eval, after, step_ms) = prefixes.evaluate(job.base_id, &job.base, &job.ops);
        let done = Done {
            base_id: job.base_id,
            ops: job.ops,
            eval,
            base: job.base,
            after,
            step_ms,
            ms: t.elapsed().as_secs_f32() * 1e3,
        };
        if out.send(done).is_err() {
            return;
        }
    }
}

impl Planner {
    /// Take the newest result if it is for this request.
    pub fn take(&mut self, base_id: u64, ops: &[Op]) -> Option<Done> {
        self.result(base_id, ops)?;
        self.last.take()
    }

    /// The newest result, if it is for this request.
    pub fn result(&self, base_id: u64, ops: &[Op]) -> Option<&Done> {
        self.last.as_ref().filter(|d| d.base_id == base_id && d.ops == ops)
    }

    /// Take results that have arrived, and hand the worker the newest
    /// request if it is free.
    pub fn poll(&mut self) {
        let mut got = Vec::new();
        if let Some(rx) = &self.rx
            && let Ok(rx) = rx.lock()
        {
            got.extend(rx.try_iter());
        }
        for d in got {
            self.busy = None;
            self.last = Some(d);
        }
        if self.busy.is_none()
            && let Some(job) = self.next.take()
        {
            self.start(job);
        }
    }

    fn start(&mut self, job: Job) {
        let Some(tx) = &self.tx else { return };
        self.busy = Some((job.base_id, job.ops.clone()));
        if tx.send(job).is_err() {
            self.busy = None;
            self.tx = None;
        }
    }

    /// Ask for `ops` on `base`; wait at most `wait` for the answer. Returns
    /// the result once it is there (now or from an earlier request).
    pub fn request(&mut self, base_id: u64, base: &Arc<World>, ops: &[Op], wait: Duration) -> Option<&Done> {
        self.poll();
        if self.result(base_id, ops).is_some() {
            return self.result(base_id, ops);
        }
        if self.tx.is_none() {
            // No worker: check it here.
            let t = Instant::now();
            let mut prefixes = plan::Prefixes::default();
            let (eval, after, step_ms) = prefixes.evaluate(base_id, base, ops);
            self.last = Some(Done {
                base_id,
                ops: ops.to_vec(),
                eval,
                base: base.clone(),
                after,
                step_ms,
                ms: t.elapsed().as_secs_f32() * 1e3,
            });
            return self.last.as_ref();
        }
        let asked = self.busy.as_ref().is_some_and(|(b, o)| *b == base_id && o == ops);
        if !asked {
            let job = Job {
                base_id,
                base: base.clone(),
                ops: ops.to_vec(),
            };
            if self.busy.is_none() {
                self.start(job);
            } else {
                self.next = Some(job);
            }
        }
        // Wait a little: an ordinary plan is ready within its own frame.
        let deadline = Instant::now() + wait;
        while self.busy.is_some() {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            let got = match self.rx.as_ref().map(|rx| rx.lock()) {
                Some(Ok(rx)) => rx.recv_timeout(left),
                _ => break,
            };
            match got {
                Ok(d) => {
                    self.busy = None;
                    self.last = Some(d);
                    if self.result(base_id, ops).is_some() {
                        break;
                    }
                    if let Some(job) = self.next.take() {
                        self.start(job);
                    }
                }
                Err(_) => break,
            }
        }
        self.result(base_id, ops)
    }

    /// Ask for `ops` on `base` and wait until it is checked (tests, and
    /// where nothing can be shown meanwhile).
    #[cfg(test)]
    pub fn request_blocking(&mut self, base_id: u64, base: &Arc<World>, ops: &[Op]) -> &Done {
        loop {
            if self.request(base_id, base, ops, Duration::from_millis(50)).is_some() {
                return self.result(base_id, ops).expect("just checked");
            }
        }
    }
}
