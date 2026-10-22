//! Keystroke latency while a process floods a buffer with output, through
//! the host and its serving loop, as a window frontend gets it: keys typed
//! at a steady pace into one buffer while `M-&` runs COMMAND (default
//! `yes`) into *Async Shell Command*, each key timed from its input to
//! the snapshot that answers it. Also how many snapshots came.
//!
//! `flood [COMMAND]`.

use std::{
    sync::{Mutex, mpsc},
    time::{Duration, Instant},
};

use techne_editor::{
    host::{Event, Host},
    present::{Input, Output},
};

const KEYS: usize = 200;
const PACE: Duration = Duration::from_millis(20);

fn main() {
    let command = std::env::args().nth(1).unwrap_or_else(|| "yes".to_string());
    let (tx, events) = mpsc::channel();
    let tx = Mutex::new(tx);
    let mut host = Host::start(None, "emacs".into(), |_| {}, move |e| drop(tx.lock().unwrap().send(e)));
    let key = |host: &mut Host, k: &str| host.send(Input::Key { key: k.to_string(), at: Instant::now() });
    let snapshots = Mutex::new(0usize);
    // Answered: when the snapshot answering each key made since `from`
    // came.
    let drain = |from: Instant, until: Option<Instant>, latencies: &mut Vec<Duration>| {
        while let Ok(e) = events.recv_timeout(until.map_or(Duration::ZERO, |u| u.saturating_duration_since(Instant::now()))) {
            if let Event::Output(Output::Snapshot(s)) = e {
                *snapshots.lock().unwrap() += 1;
                let now = Instant::now();
                latencies.extend(s.answers.iter().filter(|at| **at >= from).map(|at| now - *at));
            }
            if until.is_some_and(|u| Instant::now() >= u) {
                break;
            }
        }
    };
    let mut ignored = Vec::new();
    key(&mut host, "M-&");
    for c in command.chars() {
        key(&mut host, &if c == ' ' { "SPC".to_string() } else { c.to_string() });
    }
    key(&mut host, "RET");
    drain(Instant::now(), Some(Instant::now() + Duration::from_millis(500)), &mut ignored);
    *snapshots.lock().unwrap() = 0;
    let start = Instant::now();
    let mut latencies = Vec::new();
    for i in 0..KEYS {
        key(&mut host, "a");
        drain(start, Some(start + PACE * (i as u32 + 1)), &mut latencies);
    }
    drain(start, Some(Instant::now() + Duration::from_secs(2)), &mut latencies);
    let elapsed = start.elapsed();
    latencies.sort();
    let at =
        |q: f64| latencies.get(((latencies.len() as f64 * q) as usize).min(latencies.len().saturating_sub(1))).copied().unwrap_or_default();
    println!(
        "{command}: {} of {KEYS} keys answered; p50 {:.1} ms, p99 {:.1} ms, max {:.1} ms; {} snapshots in {:.1} s",
        latencies.len(),
        at(0.5).as_secs_f64() * 1e3,
        at(0.99).as_secs_f64() * 1e3,
        latencies.last().copied().unwrap_or_default().as_secs_f64() * 1e3,
        snapshots.lock().unwrap(),
        elapsed.as_secs_f64()
    );
}
