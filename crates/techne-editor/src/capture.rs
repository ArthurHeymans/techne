//! Cancellation at the filesystem publication boundary. Scheme writes a
//! private temporary file; a lease makes publishing it atomic with expiry or
//! caller cancellation, even if the runtime was busy when cancellation came.

use crate::server::Reply;
use std::{
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use techne_vm::{
    api::Foreign,
    vm::{Capability, Vm},
};

#[derive(Debug)]
pub struct Lease {
    pub deadline: Instant,
    state: Mutex<State>,
}

#[derive(Debug)]
enum State {
    Pending,
    Cancelled,
    Published(Reply),
}

impl Lease {
    pub fn new(timeout: Duration) -> Self {
        Self { deadline: Instant::now() + timeout, state: Mutex::new(State::Pending) }
    }

    pub fn active(&self) -> bool {
        Instant::now() < self.deadline && matches!(*self.state.lock().expect("capture lease"), State::Pending)
    }

    /// A publication that won the race is acknowledged as success, not as a
    /// timeout followed by a surprising file. Otherwise forbid publication.
    pub fn cancel(&self) -> Option<Reply> {
        let mut state = self.state.lock().expect("capture lease");
        if let State::Published(reply) = &*state {
            return Some(reply.clone());
        }
        *state = State::Cancelled;
        None
    }

    fn publish(&self, from: &Path, to: &Path, snapshot: u64, width: u32, height: u32) -> Result<(), String> {
        let mut state = self.state.lock().expect("capture lease");
        if !matches!(*state, State::Pending) {
            return Err("capture is no longer pending".into());
        }
        if Instant::now() >= self.deadline {
            *state = State::Cancelled;
            return Err("capture cancelled or expired".into());
        }
        publish(from, to)?;
        *state = State::Published(Reply::Captured { path: to.display().to_string(), snapshot, width, height });
        Ok(())
    }
}

/// Publish without replacing a name, including on filesystems lacking
/// rename-with-NOREPLACE. The private temporary file is beside the destination;
/// a hard-link fallback is atomic too, and Scheme removes its temporary link.
fn publish(from: &Path, to: &Path) -> Result<(), String> {
    #[cfg(any(target_os = "linux", target_os = "android", target_vendor = "apple", target_os = "redox"))]
    {
        use rustix::{
            fs::{CWD, RenameFlags, renameat_with},
            io::Errno,
        };
        match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
            Ok(()) => return Ok(()),
            Err(Errno::INVAL | Errno::NOSYS | Errno::OPNOTSUPP) => {}
            Err(e) => return Err(format!("{}: {e}", to.display())),
        }
    }
    std::fs::hard_link(from, to).map_err(|e| format!("{}: {e}", to.display()))
}

pub(crate) fn install(vm: &mut Vm) {
    vm.name_foreign_type::<Arc<Lease>>("capture-lease");
    vm.requiring(Capability::Files, |vm| {
        techne_vm::procedures! { vm;
            "(%capture-publish! lease from to snapshot width height)" => |lease: Option<Foreign<Arc<Lease>>>, from: String, to: String,
                snapshot: usize, width: usize, height: usize| -> Result<(), String> {
                let width = u32::try_from(width).map_err(|_| "invalid capture width")?;
                let height = u32::try_from(height).map_err(|_| "invalid capture height")?;
                if let Some(lease) = lease { lease.0.publish(Path::new(&from), Path::new(&to), snapshot as u64, width, height) }
                else { publish(Path::new(&from), Path::new(&to)) }
            };
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancellation_and_expiry_forbid_late_publication() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("temporary");
        let to = dir.path().join("frame.png");
        std::fs::write(&from, b"complete PNG").unwrap();
        let cancelled = Lease::new(Duration::from_secs(1));
        assert!(cancelled.cancel().is_none());
        assert!(cancelled.publish(&from, &to, 1, 2, 3).is_err());
        let expired = Lease::new(Duration::ZERO);
        assert!(expired.publish(&from, &to, 1, 2, 3).is_err());
        assert!(!to.exists());
    }

    #[test]
    fn publication_winning_the_race_is_reported_as_success_and_never_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let from = dir.path().join("temporary");
        let to = dir.path().join("frame.png");
        std::fs::write(&from, b"complete PNG").unwrap();
        let lease = Lease::new(Duration::from_secs(1));
        lease.publish(&from, &to, 42, 2, 3).unwrap();
        assert_eq!(lease.cancel(), Some(Reply::Captured { path: to.display().to_string(), snapshot: 42, width: 2, height: 3 }));
        assert_eq!(std::fs::read(&to).unwrap(), b"complete PNG");
        std::fs::write(&from, b"replacement").unwrap();
        assert!(lease.publish(&from, &to, 43, 2, 3).is_err());
        assert!(matches!(lease.cancel(), Some(Reply::Captured { snapshot: 42, .. })));
        assert!(Lease::new(Duration::from_secs(1)).publish(&from, &to, 43, 2, 3).is_err());
        assert_eq!(std::fs::read(&to).unwrap(), b"complete PNG");
    }
}
