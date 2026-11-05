//! Run with `nix develop .#window --command cargo test -p techne-cli
//! --test capture -- --ignored --nocapture`. Never touches the real desktop.
#![cfg(feature = "window")]

use std::{path::Path, process::Command};
use techne_editor::images::{Image, decode};

fn image(dir: &Path, name: &str) -> Image {
    let path = dir.join(name);
    decode(&path, &std::fs::read(&path).unwrap()).unwrap()
}

#[test]
#[ignore = "needs headless sway, grim, wtype and window runtime libraries"]
fn screenshots_and_bounded_recording_in_a_real_window() {
    let temp = tempfile::tempdir().unwrap();
    let dir = std::env::var_os("TECHNE_CAPTURE_ARTIFACTS").map(std::path::PathBuf::from).unwrap_or_else(|| temp.path().to_owned());
    std::fs::create_dir_all(&dir).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let binary = env!("CARGO_BIN_EXE_techne");
    let output = Command::new(binary)
        .args([
            "run",
            root.join("crates/techne-window/headless.scm").to_str().unwrap(),
            binary,
            "run",
            root.join("crates/techne-window/capture-test.scm").to_str().unwrap(),
            binary,
            dir.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}\n{}\nartifacts: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
        dir.display()
    );
    let native = image(&dir, "split.png");
    let compositor = image(&dir, "compositor.png");
    assert_eq!(native.size(), (1600, 1000));
    assert_eq!(native.size(), compositor.size());
    assert!(native.0.rgba == compositor.0.rgba, "native and compositor pixels differ");
    assert_eq!(image(&dir, "resized.png").size(), (1003, 701));
    assert_eq!(image(&dir, "interactive.png").size(), native.size());
    let popup = image(&dir, "popup.png");
    assert!(native.0.rgba != popup.0.rgba, "the minibuffer popup must be present");
    for n in 0..3 {
        assert_eq!(image(&dir, &format!("recording/{n:06}.png")).size(), native.size());
    }
    assert!(!dir.join("missing/frame.png").exists());
    assert!(!dir.read_dir().unwrap().any(|e| e.unwrap().file_name().to_string_lossy().contains(".capture-")));
    println!("Inspected captures can be retained with TECHNE_CAPTURE_ARTIFACTS; native pixels match grim exactly.");
}
