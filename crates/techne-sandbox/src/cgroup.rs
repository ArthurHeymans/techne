//! The cgroups a supervisor puts its workers in (cgroup v2): each worker
//! in one of its own, bounding its memory, processes and CPU as a whole,
//! and killed whole.
//!
//! The supervisor needs a cgroup delegated to it (a systemd service or
//! scope with `Delegate=yes`), which it is alone in: [`Cgroups::adopt`]
//! moves it into a `supervisor` leaf of it (a cgroup with processes
//! cannot give its children controllers), enables the controllers it has
//! for `workers` beside it, and each worker goes in a leaf under that
//! ([`Cgroups::worker`]), its limits written before the worker is let
//! start. Where the supervisor is not alone in its cgroup (one shared with
//! other processes, a dev machine's terminal), the limits cannot be had,
//! and `adopt` says why.

use std::{
    fs, io,
    path::{Path, PathBuf},
};

/// The cgroups of a supervisor's workers.
#[derive(Debug)]
pub struct Cgroups {
    workers: PathBuf,
    controllers: Vec<String>,
}

/// What a worker's cgroup bounds.
#[derive(Clone, Debug)]
pub struct Limits {
    /// Bytes of memory, swap none; past them the kernel kills the worker
    /// (all of it: `memory.oom.group`).
    pub memory: u64,
    /// Processes and threads.
    pub pids: u64,
    /// CPU time per period, in microseconds, if bounded: `(quota, period)`.
    pub cpu: Option<(u64, u64)>,
}

/// A worker's cgroup: removed when dropped, what is in it killed first.
#[derive(Debug)]
pub struct Worker {
    path: PathBuf,
}

impl Cgroups {
    /// Take this process's cgroup for its workers, as the module says.
    pub fn adopt() -> io::Result<Cgroups> {
        let own = own_cgroup()?;
        let procs = fs::read_to_string(own.join("cgroup.procs"))?;
        let me = std::process::id().to_string();
        let leaf = own.join("supervisor");
        // Adopted before (by an earlier call): the supervisor is in its leaf.
        if own.file_name().is_some_and(|n| n == "supervisor") {
            return Err(io::Error::other("adopted already"));
        }
        if procs.lines().any(|p| p != me) {
            return Err(io::Error::other(format!("{} has other processes: no cgroup of its own to delegate", own.display())));
        }
        let available = fs::read_to_string(own.join("cgroup.controllers"))?;
        let controllers: Vec<String> =
            ["memory", "pids", "cpu"].iter().filter(|c| available.split_whitespace().any(|a| a == **c)).map(|c| c.to_string()).collect();
        if !controllers.iter().any(|c| c == "memory") {
            return Err(io::Error::other(format!("{}: no memory controller delegated", own.display())));
        }
        mkdir(&leaf)?;
        fs::write(leaf.join("cgroup.procs"), &me)?;
        let enable = controllers.iter().map(|c| format!("+{c}")).collect::<Vec<_>>().join(" ");
        fs::write(own.join("cgroup.subtree_control"), &enable)?;
        let workers = own.join("workers");
        mkdir(&workers)?;
        fs::write(workers.join("cgroup.subtree_control"), &enable)?;
        Ok(Cgroups { workers, controllers })
    }

    /// A cgroup for a worker named `name`, bounded by `limits`. Put the
    /// worker in it ([`Worker::add`]) before it starts its work. Missing
    /// controllers or failed writes refuse it, rather than leave any
    /// configured bound unapplied.
    pub fn worker(&self, name: &str, limits: &Limits) -> io::Result<Worker> {
        let has = |c: &str| self.controllers.iter().any(|x| x == c);
        for controller in ["memory", "pids"].into_iter().chain(limits.cpu.map(|_| "cpu")) {
            if !has(controller) {
                return Err(io::Error::other(format!("no {controller} controller delegated")));
            }
        }
        let path = self.workers.join(name);
        mkdir(&path)?;
        let worker = Worker { path };
        fs::write(worker.path.join("memory.max"), limits.memory.to_string())?;
        // Swap would let it hold more than its memory, slowly.
        fs::write(worker.path.join("memory.swap.max"), "0")?;
        fs::write(worker.path.join("memory.oom.group"), "1")?;
        fs::write(worker.path.join("pids.max"), limits.pids.to_string())?;
        if let Some((quota, period)) = limits.cpu {
            fs::write(worker.path.join("cpu.max"), format!("{quota} {period}"))?;
        }
        Ok(worker)
    }
}

impl Worker {
    /// Put process `pid` in the cgroup: what it forks after is in it too.
    pub fn add(&self, pid: u32) -> io::Result<()> {
        fs::write(self.path.join("cgroup.procs"), pid.to_string())
    }

    /// Kill every process in it.
    pub fn kill(&self) -> io::Result<()> {
        fs::write(self.path.join("cgroup.kill"), "1")
    }

    /// How many times the kernel killed it for its memory.
    pub fn oom_kills(&self) -> u64 {
        fs::read_to_string(self.path.join("memory.events"))
            .ok()
            .and_then(|e| {
                e.lines().find_map(|l| l.strip_prefix("oom_group_kill ").or_else(|| l.strip_prefix("oom_kill ")).map(str::to_owned))
            })
            .and_then(|n| n.trim().parse().ok())
            .unwrap_or(0)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.kill();
        // Removed once its processes are gone, which takes a moment.
        for _ in 0..100 {
            if fs::remove_dir(&self.path).is_ok() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
}

/// This process's cgroup (v2).
fn own_cgroup() -> io::Result<PathBuf> {
    let line = fs::read_to_string("/proc/self/cgroup")?;
    let path = line.lines().find_map(|l| l.strip_prefix("0::")).ok_or_else(|| io::Error::other("no cgroup v2"))?;
    Ok(Path::new("/sys/fs/cgroup").join(path.trim().trim_start_matches('/')))
}

fn mkdir(path: &Path) -> io::Result<()> {
    match fs::create_dir(path) {
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => Ok(()),
        r => r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configured_bounds_need_their_controllers() {
        let dir = tempfile::tempdir().unwrap();
        let mut cgroups = Cgroups { workers: dir.path().into(), controllers: vec!["memory".into()] };
        let limits = Limits { memory: 1 << 20, pids: 16, cpu: Some((1000, 10000)) };
        assert!(cgroups.worker("w1", &limits).unwrap_err().to_string().contains("pids controller"));
        cgroups.controllers.push("pids".into());
        assert!(cgroups.worker("w1", &limits).unwrap_err().to_string().contains("cpu controller"));
        assert!(!dir.path().join("w1").exists(), "refuse before creating a worker cgroup");
    }
}
