//! Capture protocol and Scheme policy, without a GPU. The real renderer is
//! exercised separately by the opt-in headless CLI integration test.
use std::{
    path::Path,
    sync::mpsc,
    time::{Duration, Instant},
};
use techne_editor::{
    host::{Event, Host},
    images::{decode, encode_png},
    present::{CapturedFrame, Output},
    runtime::Runtime,
    server::{self, Reply},
};

fn runtime() -> Runtime {
    let mut rt = Runtime::with_document(techne_text::Document::new("capture me\n"), "emacs").unwrap();
    rt.eval("(import (techne editor))").unwrap();
    rt
}

#[test]
fn png_preserves_channels_dimensions_and_rejects_invalid_sizes() {
    let rgba = [255, 0, 0, 255, 0, 0, 255, 255];
    let png = encode_png(2, 1, &rgba).unwrap();
    let image = decode(Path::new("frame.png"), &png).unwrap();
    assert_eq!(image.size(), (2, 1));
    assert_eq!(image.0.rgba, rgba);
    assert!(encode_png(0, 1, &[]).is_err());
    assert!(encode_png(2, 1, &[0; 4]).is_err());
    assert!(encode_png(16384, 16384, &[]).is_err());
}

#[test]
fn scheme_bounds_jobs_and_cleans_up_after_failure() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = runtime();
    for i in 0..4 {
        rt.eval(&format!("(screenshot! (current-session) {:?})", dir.path().join(format!("{i}.png")).display().to_string())).unwrap();
    }
    assert!(rt.eval(&format!("(screenshot! (current-session) {:?})", dir.path().join("extra.png").display().to_string())).is_err());
    assert_eq!(rt.eval("(editor-take-captures! (current-session))").unwrap(), "(1 2 3 4)");
    assert_eq!(rt.eval("(editor-take-captures! (current-session))").unwrap(), "()");
    assert!(rt.eval("(editor-captured! (current-session) 1 0 0 0 #u8() \"unsupported\")").is_err());
    rt.eval(&format!("(screenshot! (current-session) {:?})", dir.path().join("extra.png").display().to_string())).unwrap();
    assert!(dir.path().read_dir().unwrap().next().is_none());
}

#[test]
fn recording_validates_limits_stops_and_does_not_replace_directories() {
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("recording");
    let path = format!("{:?}", destination.display().to_string());
    let mut rt = runtime();
    assert!(rt.eval(&format!("(recording-start! (current-session) {path} #:frames 301)")).is_err());
    assert!(rt.eval(&format!("(recording-start! (current-session) {path} #:interval 0)")).is_err());
    assert!(!destination.exists());
    rt.eval(&format!("(recording-start! (current-session) {path} #:frames 2)")).unwrap();
    assert!(rt.eval(&format!("(recording-start! (current-session) {path})")).is_err());
    rt.eval("(recording-stop! (current-session))").unwrap();
    let manifest = std::fs::read_to_string(destination.join("recording.scm")).unwrap();
    assert!(manifest.contains("(active #f)"), "{manifest}");
    assert!(rt.eval("(recording-stop! (current-session))").is_err());
    assert!(rt.eval(&format!("(recording-start! (current-session) {path})")).is_err());
}

fn serve_capture(fail: bool) {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("editor");
    let path = dir.path().join("frame.png");
    let (tx, rx) = mpsc::channel();
    let host = Host::start(
        None,
        "emacs".into(),
        |_| {},
        move |e| {
            let _ = tx.send(e);
        },
    );
    let _listener = host.listen(&socket).unwrap().unwrap();
    let (s, p) = (socket.clone(), path.clone());
    let client = std::thread::spawn(move || server::capture(&s, &p));
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut last = 0;
    loop {
        let event = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).unwrap();
        match event {
            Event::Output(Output::Snapshot(s)) => last = s.id,
            Event::Output(Output::Capture { id, after }) => {
                assert!(last >= after && after > 0);
                host.captured(
                    id,
                    if fail {
                        Err("no pixels in this frontend".into())
                    } else {
                        Ok(CapturedFrame {
                            snapshot: last,
                            width: 2,
                            height: 1,
                            png: encode_png(2, 1, &[255, 0, 0, 255, 0, 0, 255, 255]).unwrap(),
                        })
                    },
                );
                break;
            }
            Event::Failed(e) => panic!("{e}"),
            _ => {}
        }
    }
    let result = client.join().unwrap();
    if fail {
        assert!(result.unwrap_err().contains("no pixels"));
        assert!(!path.exists());
    } else {
        assert_eq!(result.unwrap(), Reply::Captured { path: path.display().to_string(), snapshot: last, width: 2, height: 1 });
        assert_eq!(decode(&path, &std::fs::read(&path).unwrap()).unwrap().size(), (2, 1));
        // Both an existing file and a dangling symlink must be refused before
        // any frontend work, preserving bytes and the symlink's target.
        assert!(server::capture(&socket, &path).unwrap_err().contains("already exists"));
        #[cfg(unix)]
        {
            let link = dir.path().join("link.png");
            std::os::unix::fs::symlink(dir.path().join("missing"), &link).unwrap();
            assert!(server::capture(&socket, &link).unwrap_err().contains("already exists"));
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
    }
    host.close();
}

#[test]
fn external_capture_waits_for_rendering_and_saving() {
    serve_capture(false);
}

#[test]
fn unsupported_frontend_returns_an_error_not_a_timeout() {
    serve_capture(true);
}

#[test]
fn missing_editor_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    assert!(server::capture(&dir.path().join("missing"), &dir.path().join("frame.png")).is_err());
}

#[test]
fn a_destination_created_during_rendering_is_not_overwritten() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("raced.png");
    let mut rt = runtime();
    rt.eval(&format!("(screenshot! (current-session) {:?})", path.display().to_string())).unwrap();
    std::fs::write(&path, "keep me").unwrap();
    assert!(rt.eval("(editor-captured! (current-session) 1 1 1 1 #u8(137 80 78 71) \"\")").is_err());
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "keep me");
    assert_eq!(dir.path().read_dir().unwrap().count(), 1, "temporary output was removed");
}

#[test]
fn recording_failure_finishes_the_manifest_and_releases_the_session() {
    let dir = tempfile::tempdir().unwrap();
    let mut rt = runtime();
    rt.eval(&format!("(recording-start! (current-session) {:?})", dir.path().join("failed").display().to_string())).unwrap();
    assert!(rt.eval("(editor-captured! (current-session) 1 0 0 0 #u8() \"unsupported\")").is_err());
    let manifest = std::fs::read_to_string(dir.path().join("failed/recording.scm")).unwrap();
    assert!(manifest.contains("(active #f)"), "{manifest}");
    assert_eq!(rt.eval("(sget (current-session) 'recording)").unwrap(), "#f");
    rt.eval(&format!("(recording-start! (current-session) {:?})", dir.path().join("next").display().to_string())).unwrap();
    std::fs::remove_dir_all(dir.path().join("next")).unwrap();
    assert!(rt.eval("(editor-captured! (current-session) 2 0 0 0 #u8() \"unsupported\")").is_err());
    assert_eq!(rt.eval("(sget (current-session) 'recording)").unwrap(), "#f", "a failed manifest write must not wedge recording");
}

#[test]
fn detaching_fails_a_waiting_capture_promptly() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("editor");
    let path = dir.path().join("frame.png");
    let (tx, rx) = mpsc::channel();
    let host = Host::start(
        None,
        "emacs".into(),
        |_| {},
        move |e| {
            let _ = tx.send(e);
        },
    );
    let _listener = host.listen(&socket).unwrap().unwrap();
    let client = std::thread::spawn(move || server::capture(&socket, &path));
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).unwrap() {
            Event::Output(Output::Capture { .. }) => break,
            Event::Failed(e) => panic!("{e}"),
            _ => {}
        }
    }
    host.close();
    assert!(client.join().unwrap().unwrap_err().contains("detached"));
}

fn wait_output(rx: &mpsc::Receiver<Event>, wanted: impl Fn(&Output) -> bool) -> Output {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())).unwrap() {
            Event::Output(o) if wanted(&o) => return o,
            Event::Failed(e) => panic!("{e}"),
            _ => {}
        }
    }
}

fn frame(snapshot: u64) -> CapturedFrame {
    CapturedFrame { snapshot, width: 1, height: 1, png: encode_png(1, 1, &[255, 0, 0, 255]).unwrap() }
}

#[test]
fn timed_out_captures_release_all_slots_and_ignore_late_worker_results() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("editor");
    let (tx, rx) = mpsc::channel();
    let host = Host::start(
        None,
        "emacs".into(),
        |_| {},
        move |e| {
            let _ = tx.send(e);
        },
    );
    let _listener = host.listen(&socket).unwrap().unwrap();
    wait_output(&rx, |o| matches!(o, Output::Snapshot(_)));
    let clients: Vec<_> = (0..4)
        .map(|n| {
            let (socket, path) = (socket.clone(), dir.path().join(format!("abandoned-{n}.png")));
            std::thread::spawn(move || server::request(&socket, &server::Request::Capture { path, timeout_ms: Some(1000) }))
        })
        .collect();
    let mut pending = Vec::new();
    for _ in 0..4 {
        if let Output::Capture { id, after } = wait_output(&rx, |o| matches!(o, Output::Capture { .. })) {
            pending.push((id, after));
        }
    }
    for client in clients {
        let error = client.join().unwrap().unwrap_err();
        assert!(error.contains("timed out"), "{error}");
    }
    for _ in 0..4 {
        wait_output(&rx, |o| matches!(o, Output::CancelCapture { .. }));
    }
    let (s, path) = (socket.clone(), dir.path().join("fresh.png"));
    let client = std::thread::spawn(move || server::capture(&s, &path));
    let Output::Capture { id, after } = wait_output(&rx, |o| matches!(o, Output::Capture { .. })) else { unreachable!() };
    assert!(pending.iter().all(|(old, _)| id > *old));
    for (old, snapshot) in pending {
        host.captured(old, Ok(frame(snapshot)));
    }
    host.captured(id, Ok(frame(after)));
    assert!(matches!(client.join().unwrap().unwrap(), Reply::Captured { .. }));
    assert!(dir.path().join("fresh.png").exists());
    for n in 0..4 {
        assert!(!dir.path().join(format!("abandoned-{n}.png")).exists());
    }
    host.close();
}

#[test]
fn a_disconnected_caller_is_cancelled_before_rendering_completes() {
    use std::{io::Write, os::unix::net::UnixStream};
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("editor");
    let path = dir.path().join("abandoned.png");
    let (tx, rx) = mpsc::channel();
    let host = Host::start(
        None,
        "emacs".into(),
        |_| {},
        move |e| {
            let _ = tx.send(e);
        },
    );
    let _listener = host.listen(&socket).unwrap().unwrap();
    wait_output(&rx, |o| matches!(o, Output::Snapshot(_)));
    let mut stream = UnixStream::connect(&socket).unwrap();
    let request = rmp_serde::to_vec_named(&server::Request::Capture { path: path.clone(), timeout_ms: None }).unwrap();
    stream.write_all(&(request.len() as u32).to_be_bytes()).unwrap();
    stream.write_all(&request).unwrap();
    let Output::Capture { id, after } = wait_output(&rx, |o| matches!(o, Output::Capture { .. })) else { unreachable!() };
    drop(stream);
    wait_output(&rx, |o| matches!(o, Output::CancelCapture { id: cancelled } if *cancelled == id));
    host.captured(id, Ok(frame(after)));
    // A following snapshot is the barrier proving the late completion was
    // processed, not just that it has not written the file yet.
    wait_output(&rx, |o| matches!(o, Output::Snapshot(_)));
    host.close();
    assert!(!path.exists());
}
