//! The watchdog: a thread that stops a slice of a task while it runs, once
//! its lifetime's CPU bound has passed.
//!
//! A lifetime's bound is charged as its tasks' slices end (`tasks.rs`). A
//! slice that does not end, Lisp looping where its task cannot be
//! preempted (a `define-library` body, `eval`, a native calling back),
//! would never be charged, and would hold its thread and every world on it.
//! So while a world runs a slice of a task with a CPU bound, its slot here
//! says which execution and when the bound passes, in the CPU time of the
//! world's thread; the watchdog reads that thread's CPU clock (another
//! thread's can be read: `pthread_getcpuclockid`) and asks for a kill past
//! it. Kills are taken where code cannot suspend too, so the slice ends,
//! is charged, and its lifetime ends past its bound as at any other time.
//!
//! The watchdog wakes every `TICK` while a slot is armed, and sleeps while
//! none is: arming costs a lock and a few stores, and a wake only when it
//! sleeps. A Rust native that never looks for stops is beyond it: a kill
//! it asked for that the slice has not ended at a second after
//! (`GRACE`) is told to the process's hook (`stop::on_undelivered_stop`),
//! for a worker to end itself, its supervisor to see to the rest.

use std::{
    sync::{
        Arc, Mutex, OnceLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
    thread::Thread,
    time::Duration,
};

use crate::stop::{ExecId, InterruptHandle, Stop};

/// How often the watchdog looks while a slice is watched: how far past its
/// bound a slice it stops may run.
const TICK: Duration = Duration::from_millis(2);

/// How long after a kill is asked for its slice must have ended: a native
/// ends within about 100 ms (`runtime/R7RS.md` bounds numbers' work), and
/// one that has not by then does not look for stops.
const GRACE: Duration = Duration::from_secs(1);

/// What to call when a kill is not delivered within `GRACE`.
pub(crate) static UNDELIVERED: OnceLock<Box<dyn Fn() + Send + Sync>> = OnceLock::new();

/// A world's slot: its thread's CPU clock, how to stop its executions,
/// and what to stop when, if anything.
pub(crate) struct Slot {
    #[cfg(target_os = "linux")]
    clock: libc::clockid_t,
    stop: InterruptHandle,
    /// The execution, and the CPU time of the thread past which it is
    /// stopped.
    armed: Mutex<Option<(ExecId, Duration)>>,
    /// When a kill was asked for, while its slice has not ended.
    asked: Mutex<Option<std::time::Instant>>,
}

impl Slot {
    /// A slot for the world `stop` stops, on this thread.
    pub(crate) fn new(stop: InterruptHandle) -> Arc<Slot> {
        #[cfg(target_os = "linux")]
        let clock = {
            let mut clock: libc::clockid_t = 0;
            // SAFETY: this thread, and a clock id to write to.
            unsafe { libc::pthread_getcpuclockid(libc::pthread_self(), &mut clock) };
            clock
        };
        let slot = Arc::new(Slot {
            #[cfg(target_os = "linux")]
            clock,
            stop,
            armed: Mutex::new(None),
            asked: Mutex::new(None),
        });
        watchdog().slots.lock().unwrap_or_else(|e| e.into_inner()).push(Arc::downgrade(&slot));
        slot
    }

    /// Stop `exec` once the thread has used `left` more CPU time, given
    /// `(exec, left)`; given `None`, stop nothing.
    pub(crate) fn set(&self, target: Option<(ExecId, Duration)>) {
        let mut armed = self.armed.lock().unwrap_or_else(|e| e.into_inner());
        // The slice the world sets this after has ended: a kill asked for
        // was delivered.
        *self.asked.lock().unwrap_or_else(|e| e.into_inner()) = None;
        // Nothing to watch, nor to stop watching: no clock to read.
        if target.is_none() && armed.is_none() {
            return;
        }
        *armed = target.map(|(exec, left)| (exec, self.cpu().saturating_add(left)));
        drop(armed);
        let w = watchdog();
        if target.is_some() && w.sleeping.swap(false, Ordering::SeqCst) {
            w.thread.unpark();
        }
    }

    /// The CPU time the world's thread has used.
    fn cpu(&self) -> Duration {
        #[cfg(target_os = "linux")]
        {
            let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
            // SAFETY: a clock id of a thread (one that ended reads as an
            // error, leaving zero), and a timespec to write to.
            unsafe { libc::clock_gettime(self.clock, &mut ts) };
            Duration::new(ts.tv_sec as u64, ts.tv_nsec as u32)
        }
        // Elsewhere, wall time, as lifetimes count it there.
        #[cfg(not(target_os = "linux"))]
        {
            static START: OnceLock<std::time::Instant> = OnceLock::new();
            START.get_or_init(std::time::Instant::now).elapsed()
        }
    }

    /// Stop what is armed if it is past its end; whether something is
    /// armed still. The stop is asked for holding the slot, so that the
    /// world cannot set another watch for the execution meanwhile (its next
    /// slice) that the stop would then end early. A notifier of the world's
    /// panicking does not end the watchdog: it watches every world.
    fn look(&self) -> bool {
        let mut armed = self.armed.lock().unwrap_or_else(|e| e.into_inner());
        let mut asked = self.asked.lock().unwrap_or_else(|e| e.into_inner());
        let Some((exec, end)) = *armed else {
            // A kill asked for: watched until its slice ends, or past
            // `GRACE`, when the hook is told (once).
            return match *asked {
                Some(at) if at.elapsed() >= GRACE => {
                    *asked = None;
                    if let Some(hook) = UNDELIVERED.get() {
                        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(hook));
                    }
                    false
                }
                Some(_) => true,
                None => false,
            };
        };
        if self.cpu() < end {
            return true;
        }
        *armed = None;
        *asked = Some(std::time::Instant::now());
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.stop.stop(exec, Stop::Kill)));
        true
    }
}

struct Watchdog {
    slots: Mutex<Vec<Weak<Slot>>>,
    thread: Thread,
    /// Parked until a slot is armed.
    sleeping: AtomicBool,
}

fn watchdog() -> &'static Watchdog {
    static WATCHDOG: OnceLock<Watchdog> = OnceLock::new();
    WATCHDOG.get_or_init(|| {
        let thread = std::thread::Builder::new()
            .name("techne-watchdog".into())
            .spawn(|| {
                let w = watchdog();
                loop {
                    let slots: Vec<Arc<Slot>> = {
                        let mut slots = w.slots.lock().unwrap_or_else(|e| e.into_inner());
                        slots.retain(|s| s.strong_count() > 0);
                        slots.iter().filter_map(Weak::upgrade).collect()
                    };
                    // Each looked at: one past its end is stopped.
                    let mut watching = false;
                    for s in &slots {
                        watching |= s.look();
                    }
                    drop(slots);
                    if watching {
                        std::thread::sleep(TICK);
                        continue;
                    }
                    // Asleep, unless a slot was armed since it looked.
                    w.sleeping.store(true, Ordering::SeqCst);
                    let armed = w.slots.lock().unwrap_or_else(|e| e.into_inner()).iter().filter_map(Weak::upgrade).any(|s| {
                        s.armed.lock().unwrap_or_else(|e| e.into_inner()).is_some()
                            || s.asked.lock().unwrap_or_else(|e| e.into_inner()).is_some()
                    });
                    if !armed {
                        std::thread::park();
                    }
                    w.sleeping.store(false, Ordering::SeqCst);
                }
            })
            .expect("the watchdog's thread");
        Watchdog { slots: Mutex::new(Vec::new()), thread: thread.thread().clone(), sleeping: AtomicBool::new(false) }
    })
}
