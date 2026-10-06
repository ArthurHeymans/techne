//! `techne-compositor [--nested]`: run the compositor on this TTY (DRM), or
//! nested in a window of the current session for development.
//!
//! No policy owner connects yet, so the events it would receive are logged.

use std::time::Duration;

use techne_compositor::{backend, cursor::CursorConfig, policy};

fn main() {
    let nested = match std::env::args().nth(1).as_deref() {
        None => false,
        Some("--nested") => true,
        Some(other) => {
            eprintln!("usage: techne-compositor [--nested]  (unknown argument {other})");
            std::process::exit(2);
        }
    };
    policy::init_logging(nested);
    policy::init_event_channel(Box::new(std::io::sink()));
    std::thread::spawn(|| {
        loop {
            for event in policy::drain_events() {
                tracing::info!("event: {event:?}");
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    });
    let cursor = CursorConfig::default();
    let result = if nested {
        backend::winit::run_winit(cursor)
    } else {
        backend::drm::run_drm(cursor)
    };
    if let Err(e) = result {
        eprintln!("techne-compositor: {e}");
        std::process::exit(1);
    }
}
