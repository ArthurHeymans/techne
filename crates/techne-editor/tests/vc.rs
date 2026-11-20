//! The vertical slice of the architecture review: a version control status
//! view (`lisp/editor/packages/vc.scm`) whose rows come from a process in
//! the background, keep the caret on its row as they change, reload with
//! their package while work is in flight, and whose targets' actions work
//! wherever the targets come from.

use std::{
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

use techne_editor::{
    present::{Input, Snapshot},
    runtime::{Runtime, lisp_dir},
};

fn keys(rt: &mut Runtime, keys: &str) {
    for k in keys.split(' ') {
        rt.handle(Input::Key { key: k.to_string(), at: Instant::now() });
    }
}

fn type_text(rt: &mut Runtime, text: &str) {
    for c in text.chars() {
        let key = if c == ' ' { "SPC".to_string() } else { c.to_string() };
        rt.handle(Input::Key { key, at: Instant::now() });
    }
}

/// Run background tasks until `done` holds of a snapshot, as the host's
/// loop does between inputs.
fn until(rt: &mut Runtime, what: &str, done: impl Fn(&Snapshot) -> bool) -> Snapshot {
    let start = Instant::now();
    loop {
        rt.run_tasks(Duration::from_millis(5)).unwrap();
        let s = rt.snapshot();
        if done(&s) {
            return s;
        }
        if start.elapsed() >= Duration::from_secs(20) {
            let failures = rt.eval("(map (lambda (r) (let ((c (record-condition r))) (and c (error-text c)))) (kept-operation-records))");
            let prompt = s.minibuffer.as_ref().map(|m| m.prompt.clone());
            panic!(
                "timed out waiting for {what}: {:?} / {} / minibuffer {prompt:?} / failures {failures:?}",
                s.pane().text.to_string(),
                s.echo
            );
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn git(dir: &Path, args: &[&str]) {
    let ok = Command::new("git")
        .arg("-C")
        .arg(dir)
        // The user's signing settings would ask for a key.
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com", "-c", "init.defaultBranch=main", "-c", "commit.gpgsign=false"])
        .args(args)
        .output()
        .unwrap()
        .status
        .success();
    assert!(ok, "git {args:?}");
}

/// A Git repository with a.txt and b.txt committed and a.txt changed, and
/// a runtime on a.txt (its journal outside the repository).
fn repository() -> (tempfile::TempDir, tempfile::TempDir, PathBuf, Runtime) {
    let (dir, state) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let root = dir.path().canonicalize().unwrap();
    git(&root, &["init", "-q"]);
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    std::fs::write(root.join("b.txt"), "two\n").unwrap();
    git(&root, &["add", "."]);
    git(&root, &["commit", "-q", "-m", "start"]);
    std::fs::write(root.join("a.txt"), "ONE\n").unwrap();
    let rt = Runtime::open(&root.join("a.txt"), &state.path().join("a.journal"), "emacs").unwrap().0;
    (dir, state, root, rt)
}

fn text(s: &Snapshot) -> String {
    s.pane().text.to_string()
}

/// Move the caret to the start of the line holding the `nth` (from 0)
/// occurrence of `needle` in the focused pane.
fn goto(rt: &mut Runtime, needle: &str, nth: usize) {
    let t = text(&rt.snapshot());
    let at = t.match_indices(needle).nth(nth).unwrap_or_else(|| panic!("no {needle:?} #{nth} in {t}")).0;
    let start = t[..at].rfind('\n').map_or(0, |i| i + 1);
    rt.eval(&format!("(move! (current-session) (lambda (_) {start}))")).unwrap();
}

/// The text of the pane showing the buffer `name`, if one does.
fn pane_named(s: &Snapshot, name: &str) -> Option<String> {
    s.panes.iter().find(|p| p.status.contains(name)).map(|p| p.text.to_string())
}

#[test]
fn file_buffer_shortcuts_inspect_the_exact_git_path() {
    let (_dir, _state, root, mut rt) = repository();
    git(&root, &["add", "a.txt"]);
    std::fs::write(root.join("a.txt"), "ONE\nlocal edit\n").unwrap();
    std::fs::write(root.join("b.txt"), "different file\n").unwrap();
    keys(&mut rt, "C-x v =");
    let compare =
        until(&mut rt, "choose the index or working copy", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Compare a.txt")));
    assert_eq!(compare.minibuffer.unwrap().rows.len(), 2);
    type_text(&mut rt, "Index → working");
    keys(&mut rt, "RET");
    let patch = until(&mut rt, "file buffer patch", |s| s.panes.iter().any(|p| p.text.to_string().contains("+local edit")));
    assert!(!patch.panes.iter().any(|p| p.text.to_string().contains("+different file")));
    keys(&mut rt, "C-x v l");
    until(&mut rt, "history with the original commit", |s| text(s).contains("File history · a.txt") && text(s).contains("start"));
    keys(&mut rt, "n RET");
    let patch = until(&mut rt, "only the selected file in this commit", |s| s.panes.iter().any(|p| p.text.to_string().contains("+one")));
    assert!(!patch.panes.iter().any(|p| p.text.to_string().contains("+two")));
    keys(&mut rt, "q C-x v g");
    until(&mut rt, "annotations for the saved Git file", |s| text(s).contains("Git annotate · a.txt") && text(s).contains("local edit"));
    git(&root, &["commit", "-qm", "Stage only"]);
    keys(&mut rt, "q C-x v g");
    let annotated = until(&mut rt, "annotations after commit", |s| text(s).contains("Git annotate · a.txt") && text(s).contains("ONE"));
    assert!(text(&annotated).contains("uncommitted"));
}

#[cfg(unix)]
#[test]
fn forge_pages_and_compares_pinned_objects() {
    const CHILD: &str = "TECHNE_FORGE_TEST_CHILD";
    let base = "a".repeat(40);
    let head = "b".repeat(40);
    if std::env::var_os(CHILD).is_none() {
        use std::os::unix::fs::PermissionsExt;

        // Pass the fixture transport only to this test's own subprocess.
        let bin = tempfile::tempdir().unwrap();
        let curl = bin.path().join("curl");
        let pr = |number: u8, title: &str| {
            format!(
                r#"[{{"number":{number},"title":"{title}","state":"open","draft":false,"user":{{"login":"author"}},"head":{{"sha":"{head}","label":"author:topic"}},"base":{{"sha":"{base}","ref":"main"}}}}]"#
            )
        };
        std::fs::write(
            &curl,
            format!(r#"#!/bin/sh
head_filter=""
for last; do case "$last" in head=*) head_filter="$last" ;; esac; done
case "$head_filter" in
  'head=octocat:topic/scoped')
    [ "$last" = 'https://api.github.com/repos/octocat/project/pulls?state=open&per_page=30&page=1' ] || exit 1
    printf '%s' '[{{"number":3,"title":"Right source","head":{{"ref":"topic/scoped","repo":{{"full_name":"Octocat/Project"}}}}}},{{"number":4,"title":"Wrong fork","head":{{"ref":"topic/scoped","repo":{{"full_name":"Other/Project"}}}}}}]'
    exit 0 ;;
  '') ;;
  *) exit 1 ;;
esac
case "$last" in
 'https://api.github.com/repos/octocat/project/pulls?state=open&per_page=30&page=1') printf '%s' '{}' ;;
 'https://api.github.com/repos/octocat/project/pulls?state=open&per_page=30&page=2') printf '%s' '{}' ;;
 'https://api.github.com/repos/octocat/project/compare/{base}...{head}?per_page=1') printf '%s' '{{"files":[{{"filename":"a.txt","patch":"@@ -1 +1 @@\n-one\n+ONE"}},{{"filename":"b.txt","patch":null}}]}}' ;;
 'https://api.github.com/repos/octocat/project/issues/2/comments?per_page=30&page=1') printf '%s' '[{{"user":{{"login":"reviewer"}},"body":"General feedback"}}{general_tail}]' ;;
 'https://api.github.com/repos/octocat/project/pulls/2/comments?per_page=30&page=1') printf '%s' '[{{"user":{{"login":"reviewer"}},"body":"On older code","path":"a.txt","commit_id":"{base}"}},{{"user":{{"login":"reviewer"}},"body":"On current code","path":"b.txt","commit_id":"{head}"}}]' ;;
 'https://api.github.com/repos/octocat/project/issues/2/comments?per_page=30&page=2') printf '%s' '[{{"user":{{"login":"reviewer"}},"body":"Later feedback"}}]' ;;
 'https://api.github.com/repos/octocat/project/pulls/2/comments?per_page=30&page=2') printf '%s' '[]' ;;
 'https://api.github.com/repos/octocat/project/commits/{head}/check-runs?per_page=30&page=1') printf '%s' '{{"total_count":31,"check_runs":[{{"name":"unit tests","status":"completed","conclusion":"success"}},{{"name":"integration","status":"queued","conclusion":null}}]}}' ;;
 'https://api.github.com/repos/octocat/project/commits/{head}/check-runs?per_page=30&page=2') printf '%s' '{{"total_count":31,"check_runs":[{{"name":"release gate","status":"completed","conclusion":"failure"}}]}}' ;;
 *) printf '%s\n' "Unexpected forge request: $last" >&2; exit 1 ;;
esac
"#, pr(1, "First page"), pr(2, "Second page"), general_tail = r#",{"body":"Earlier discussion"}"#.repeat(29)),
        )
        .unwrap();
        std::fs::set_permissions(&curl, std::fs::Permissions::from_mode(0o700)).unwrap();
        let path = std::env::join_paths(
            [bin.path().to_path_buf()].into_iter().chain(std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())),
        )
        .unwrap();
        let run = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "forge_pages_and_compares_pinned_objects", "--nocapture"])
            .env(CHILD, "1")
            .env("PATH", path)
            .output()
            .unwrap();
        assert!(run.status.success(), "{}\n{}", String::from_utf8_lossy(&run.stdout), String::from_utf8_lossy(&run.stderr));
        return;
    }

    let (_repo, _state, root, mut rt) = repository();
    git(&root, &["remote", "add", "origin", "git@github.com:Octocat/Project.git"]);
    git(&root, &["switch", "-qc", "topic/scoped"]);
    keys(&mut rt, "C-x v d");
    until(&mut rt, "Git status", |s| text(s).contains("Head:     topic/scoped"));
    keys(&mut rt, "N b");
    let scoped = until(&mut rt, "matching PRs for this branch", |s| text(s).contains("#3  Right source"));
    assert!(text(&scoped).contains("Published source: octocat/project:topic/scoped"));
    assert!(!text(&scoped).contains("Wrong fork"));
    keys(&mut rt, "q N l");
    until(&mut rt, "first forge page", |s| text(s).contains("#1  First page"));
    keys(&mut rt, "]");
    until(&mut rt, "second forge page", |s| text(s).contains("#2  Second page"));
    keys(&mut rt, "M-< n RET");
    let review = until(&mut rt, "published comparison", |s| text(s).contains("Published"));
    let first_review = rt.eval("(buffer-name (session-buffer (current-session)))").unwrap();
    assert!(text(&review).contains(&format!("{base} → {head}")));
    assert!(text(&review).contains("#2 · author · author:topic → main"));
    assert!(text(&review).contains("+ONE"));
    assert!(text(&review).contains("Patch unavailable"));
    keys(&mut rt, "n n");
    let review = rt.snapshot();
    let at = review.pane().head();
    assert!(text(&review)[at..].starts_with("b.txt"));
    keys(&mut rt, "d");
    let discussion = until(&mut rt, "review discussion", |s| text(s).contains("General feedback"));
    assert!(text(&discussion).contains(&format!("other revision {base}")));
    assert!(text(&discussion).contains("on displayed head"));
    keys(&mut rt, "]");
    let next = until(&mut rt, "second discussion page", |s| text(s).contains("Later feedback"));
    assert!(text(&next).contains("discussion · page 2"));
    keys(&mut rt, "]");
    assert_eq!(rt.snapshot().echo, "No further comments");
    keys(&mut rt, "[");
    until(&mut rt, "back to first discussion page", |s| text(s).contains("General feedback"));
    keys(&mut rt, "C");
    let checks = until(&mut rt, "checks pinned to the reviewed head", |s| text(s).contains("unit tests"));
    assert!(text(&checks).contains("completed · success"));
    assert!(text(&checks).contains("integration  queued"));
    keys(&mut rt, "]");
    let next_checks = until(&mut rt, "second checks page for the same head", |s| text(s).contains("release gate"));
    assert!(text(&next_checks).contains("page 2") && !text(&next_checks).contains("unit tests"));
    keys(&mut rt, "]");
    assert_eq!(rt.snapshot().echo, "No further check runs");
    keys(&mut rt, "[");
    until(&mut rt, "back to first check runs", |s| text(s).contains("unit tests"));
    keys(&mut rt, "q q q");
    let returned = rt.snapshot();
    assert_eq!(returned.panes.len(), 1, "{} / {}", text(&returned), returned.echo);
    assert!(text(&returned).contains("#2  Second page"));
    keys(&mut rt, "[");
    until(&mut rt, "back on the first page", |s| text(s).contains("#1  First page"));
    keys(&mut rt, "q");
    assert!(text(&rt.snapshot()).contains("Git status"));

    let other = tempfile::tempdir().unwrap();
    let file = other.path().join("other.txt");
    git(other.path(), &["init", "-q"]);
    git(other.path(), &["remote", "add", "origin", "git@github.com:Octocat/Project.git"]);
    std::fs::write(&file, "different checkout\n").unwrap();
    rt.eval(&format!("(show-document! (current-session) (file-document {:?}))", file.to_str().unwrap())).unwrap();
    keys(&mut rt, "C-x v d");
    until(&mut rt, "second checkout", |s| text(s).contains("other.txt"));
    keys(&mut rt, "N l");
    until(&mut rt, "first page in second checkout", |s| text(s).contains("#1  First page"));
    keys(&mut rt, "]");
    until(&mut rt, "second page in second checkout", |s| text(s).contains("#2  Second page"));
    keys(&mut rt, "M-< n RET");
    until(&mut rt, "second checkout's comparison", |s| text(s).contains("Published"));
    assert_ne!(rt.eval("(buffer-name (session-buffer (current-session)))").unwrap(), first_review);
    assert_eq!(rt.eval(&format!("(and (buffer-named {first_review}) #t)")).unwrap(), "#t");
    keys(&mut rt, "q q");
    assert!(text(&rt.snapshot()).contains("other.txt"));

    if Command::new("jj").arg("version").output().is_ok_and(|out| out.status.success()) {
        let jj_repo = tempfile::tempdir().unwrap();
        let root = jj_repo.path();
        let run_jj = |args: &[&str]| {
            let out = Command::new("jj").current_dir(root).args(args).output().unwrap();
            assert!(out.status.success(), "jj {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        };
        run_jj(&["git", "init", "--no-colocate"]);
        run_jj(&["git", "remote", "add", "origin", "https://github.com/octocat/project.git"]);
        let path = root.join("jj-file.txt");
        std::fs::write(&path, "recorded change\n").unwrap();
        run_jj(&["--config", "user.name='Editor Test'", "--config", "user.email='test@example.test'", "describe", "-m", "Jj working copy"]);
        run_jj(&["bookmark", "set", "-r", "@", "topic/scoped"]);
        rt.eval(&format!("(show-document! (current-session) (file-document {:?}))", path.to_str().unwrap())).unwrap();
        keys(&mut rt, "C-x v d");
        until(&mut rt, "Jujutsu graph", |s| text(s).contains("Jj working copy"));
        keys(&mut rt, "M-< n N b");
        let scoped = until(&mut rt, "matching PRs for the selected jj bookmark", |s| text(s).contains("#3  Right source"));
        assert!(text(&scoped).contains("Published source: octocat/project:topic/scoped"));
        assert!(!text(&scoped).contains("Wrong fork"));
    }
}

#[test]
fn a_status_view_from_a_process() {
    let (_dir, _state, root, mut rt) = repository();
    rt.eval("(set-option! 'vc-refresh-interval #f)").unwrap();
    rt.eval("(set-option! 'vc-show-hints #f)").unwrap();
    let out = Command::new("git").current_dir(&root).args(["log", "-1", "--format=%h"]).output().unwrap();
    assert!(out.status.success());
    let sha = String::from_utf8(out.stdout).unwrap();
    let sha = sha.trim();
    let head = format!("Git status\n{}\nHead:     main  {sha} start", root.display());
    let recent = format!("▾ Recent commits (1)\n  {sha} main start");
    keys(&mut rt, "C-x v d");
    let s = until(&mut rt, "the status", |s| text(s).contains("a.txt"));
    assert_eq!(text(&s), format!("{head}\n▾ Unstaged changes (1)\n  ▸ a.txt\n{recent}"));
    assert!(s.pane().status.contains("vc-git-status"), "{}", s.pane().status);
    // M-n goes from section to section; TAB folds one.
    keys(&mut rt, "M-n TAB");
    let folded = rt.snapshot();
    assert!(text(&folded).contains("▸ Unstaged changes (1)\n▾ Recent"), "{} / {}", text(&folded), folded.echo);
    keys(&mut rt, "TAB");
    let reopened = rt.snapshot();
    assert!(text(&reopened).contains("  ▸ a.txt\n"), "{} / {}", text(&reopened), reopened.echo);
    // The caret inside a.txt's row; then more changes, one wider.
    keys(&mut rt, "n C-f C-f C-f C-f C-f");
    std::fs::write(root.join("b.txt"), "TWO\n").unwrap();
    std::fs::write(root.join("c.txt"), "new\n").unwrap();
    keys(&mut rt, "g");
    let s = until(&mut rt, "the refresh", |s| text(s).contains("c.txt"));
    assert_eq!(text(&s), format!("{head}\n▾ Untracked files (1)\n  ▸ c.txt\n▾ Unstaged changes (2)\n  ▸ a.txt\n  ▸ b.txt\n{recent}"));
    let (t, head_at) = (&s.pane().text, s.pane().head());
    assert_eq!(t.byte_to_line(head_at), 6, "still on a.txt's row");
    assert_eq!(t.byte_slice(head_at - 1..head_at).to_string(), "a", "after the same character");
    // RET opens the file of the row.
    keys(&mut rt, "C-n RET");
    assert!(rt.snapshot().pane().status.contains("b.txt"));
}

#[test]
fn inline_and_separate_patches_show_source_faces_and_keep_diff_signs() {
    let (_dir, _state, root, mut rt) = repository();
    let file = root.join("example.rs");
    std::fs::write(&file, "fn greeting() -> &'static str {\n    let word = \"café\";\n    word\n}\n").unwrap();
    git(&root, &["add", "example.rs"]);
    git(&root, &["commit", "-qm", "Greeting"]);
    std::fs::write(&file, "fn greeting() -> &'static str {\n    let word = \"cafés\";\n    word\n}\n").unwrap();
    rt.eval("(set-face! 'keyword #:fg \"#123456\")").unwrap();
    keys(&mut rt, "C-x v d");
    until(&mut rt, "collapsed file", |s| text(s).contains("▸ example.rs"));
    goto(&mut rt, "▸ example.rs", 0);
    keys(&mut rt, "TAB");
    let added = "+    let word = \"cafés\";";
    let inline = until(&mut rt, "expanded file", |s| text(s).contains("▾ example.rs") && text(s).contains(added));
    let check = |s: &Snapshot, pane: &techne_editor::present::Pane| {
        let text = pane.text.to_string();
        let style_at = |at| s.style(pane.layers.iter().find(|h| h.from <= at && at < h.to).unwrap().style);
        let start = text.find(added).unwrap();
        assert_eq!(style_at(start + 5).fg.unwrap().rgb, [0x12, 0x34, 0x56]);
        assert_ne!(style_at(start).fg, style_at(start + 5).fg);
        let added_band = style_at(start).bg.expect("added row background");
        assert_eq!(style_at(start + 5).bg, Some(added_band), "syntax overlays the band");
        assert_eq!(style_at(start + added.len()).extend, Some(true), "the band reaches past the text");
        assert_ne!(style_at(start + added.find("cafés").unwrap()).bg, Some(added_band), "word emphasis is stronger");
        let old = text.find("-    let").unwrap();
        let removed_band = style_at(old).bg.expect("removed row background");
        assert_eq!(style_at(old + 5).bg, Some(removed_band));
        assert_ne!(added_band, removed_band);
        assert_eq!(style_at(old + 5).fg, style_at(start + 5).fg);
        assert_ne!(style_at(old).fg, style_at(start).fg);
        assert!(
            !pane
                .layers
                .iter()
                .any(|h| h.from <= start + added.len() + 2 && start + added.len() + 2 < h.to && s.style(h.style).bg.is_some())
        );
    };
    check(&inline, inline.pane());
    keys(&mut rt, "TAB");
    assert!(text(&rt.snapshot()).contains("▸ example.rs"));
    assert!(!text(&rt.snapshot()).contains(added));
    keys(&mut rt, "=");
    let separate = until(&mut rt, "separate patch", |s| s.panes.iter().any(|p| p.text.to_string().contains(added)));
    check(&separate, separate.panes.iter().find(|p| p.text.to_string().contains(added)).unwrap());
}

#[test]
fn log_columns_keep_hashes_and_metadata_aligned_across_a_merge_and_resize() {
    let (_dir, _state, root, mut rt) = repository();
    git(&root, &["switch", "-qc", "side"]);
    git(&root, &["commit", "-qam", "Short side subject", "--author=Side Author <side@example.com>"]);
    git(&root, &["switch", "-q", "main"]);
    git(&root, &["commit", "-q", "--allow-empty", "-m", "Primary subject", "--author=Main Author <main@example.com>"]);
    git(&root, &["merge", "-q", "--no-ff", "side", "-m", "Merge the side"]);
    rt.handle(Input::Viewport { columns: 120, rows: 25, split_gap: 1 });
    keys(&mut rt, "C-x v d");
    until(&mut rt, "status", |s| text(s).contains("Merge the side"));
    keys(&mut rt, "l l");
    let log = until(&mut rt, "aligned log", |s| text(s).contains("Side Author"));
    let log = text(&log);
    let main = log.lines().find(|l| l.contains("Primary subject")).unwrap();
    let side = log.lines().find(|l| l.contains("Short side subject")).unwrap();
    assert_eq!(main.find("Main Author"), side.find("Side Author"), "{log}");
    assert_eq!(main.len(), side.len(), "ages share a right edge");
    let sha = git_out(&root, &["rev-parse", "--short", "side"]);
    assert!(side.starts_with(sha.trim()), "hash at left: {side}");
    assert!(log.lines().any(|l| l.starts_with(&" ".repeat(sha.trim().len() + 1)) && l.trim() == "|\\"), "{log}");
    goto(&mut rt, "Short side subject", 0);
    rt.handle(Input::Viewport { columns: 54, rows: 25, split_gap: 1 });
    let narrow = rt.snapshot();
    let narrow_text = text(&narrow);
    assert!(!narrow_text.contains("Side Author"), "{narrow_text}");
    assert!(narrow_text.contains("Short side subject"));
    assert!(narrow.pane().text.line(narrow.pane().text.byte_to_line(narrow.pane().head())).to_string().starts_with(sha.trim()));
}

#[test]
fn history_help_preview_and_focused_inspection() {
    let (_dir, _state, root, mut rt) = repository();
    std::fs::write(root.join("b.txt"), "TWO\n").unwrap();
    git(&root, &["commit", "-qam", "Two files"]);
    keys(&mut rt, "C-x v d");
    until(&mut rt, "Git status", |s| text(s).contains("Two files"));
    keys(&mut rt, "l l");
    until(&mut rt, "history", |s| text(s).contains("Commits in HEAD"));
    keys(&mut rt, "?");
    let empty_menu = pane_named(&rt.snapshot(), "*vc-menu*").unwrap();
    assert!(empty_menu.contains("Next entry") && !empty_menu.contains("Open change"), "{empty_menu}");
    keys(&mut rt, "C-g");
    goto(&mut rt, "Two files", 0);
    let before = rt.snapshot();
    keys(&mut rt, "?");
    let menu = pane_named(&rt.snapshot(), "*vc-menu*").unwrap();
    assert!(menu.contains("Open change") && menu.contains("Copy ID") && menu.contains("Cherry-pick"), "{menu}");
    assert!(!menu.contains("Stage") && !menu.contains("Discard"), "{menu}");
    keys(&mut rt, "C-g SPC");
    let preview = until(&mut rt, "side preview", |s| s.panes.iter().any(|p| p.text.to_string().contains("+TWO")));
    assert_eq!(preview.pane().view, before.pane().view);
    let other_pane = preview.panes.iter().find(|p| p.view != preview.pane().view).unwrap();
    let diff = other_pane.text.to_string();
    let bar = diff.find(" | 2 +-").unwrap() + " | 2 ".len();
    let rgb = |at| {
        let h = other_pane.layers.iter().find(|h| h.from <= at && at < h.to).unwrap();
        preview.style(h.style).fg.unwrap().rgb
    };
    assert_eq!(rgb(bar), [0xa6, 0xe3, 0xa1]);
    assert_eq!(rgb(bar + 1), [0xf3, 0x8b, 0xa8]);
    for (key, screens) in [("M-<next>", 1.0), ("M-<prior>", -1.0)] {
        keys(&mut rt, key);
        let paged = rt.snapshot();
        assert_eq!(paged.pane().view, before.pane().view);
        assert_eq!(paged.pane().head(), before.pane().head());
        assert!(paged.pane().request.is_none());
        let pane = paged.panes.iter().find(|p| p.view == other_pane.view).unwrap();
        assert_eq!(pane.request, Some(techne_editor::present::ViewRequest::Page { screens, context: 2 }));
        let anchor = if screens > 0.0 { diff.find("\ndiff --git").unwrap() + 1 } else { 0 };
        rt.handle(Input::Scroll { view: pane.view, revision: pane.revision, anchor, caret: Some(anchor) });
        let answered = rt.snapshot();
        assert_eq!(answered.pane().view, before.pane().view);
        assert_eq!(answered.pane().head(), before.pane().head());
        assert_eq!(answered.panes.iter().find(|p| p.view == pane.view).unwrap().scroll, anchor);
    }
    keys(&mut rt, "RET");
    let opened = until(&mut rt, "focused read-only change", |s| text(s).contains("+TWO"));
    assert_eq!(opened.panes.len(), preview.panes.len());
    let patch = text(&opened);
    let headers = patch.match_indices("\n@@ ").map(|(i, _)| i + 1).collect::<Vec<_>>();
    assert_eq!(headers.len(), 2);
    for (key, target) in [("n", headers[0]), ("n", headers[1]), ("p", headers[0])] {
        keys(&mut rt, key);
        assert_eq!(rt.snapshot().pane().head(), target);
    }
    rt.eval(&format!("(view-set-scroll! (session-view (current-session)) {})", headers[0])).unwrap();
    assert_ne!(rt.snapshot().pane().scroll, 0);
    keys(&mut rt, "q");
    let back = rt.snapshot();
    assert_eq!(back.pane().view, before.pane().view);
    assert_eq!(back.pane().head(), before.pane().head());
    assert_eq!(back.pane().scroll, before.pane().scroll);

    // Opening this change again must not inherit the patch's scroll.
    keys(&mut rt, "RET");
    let reopened = until(&mut rt, "reopened change", |s| text(s).contains("+TWO"));
    assert_eq!(reopened.pane().scroll, 0);
    assert_eq!(reopened.pane().selections, [(0, 0)]);
    rt.eval(&format!("(view-set-scroll! (session-view (current-session)) {})", headers[0])).unwrap();
    keys(&mut rt, "q n RET");
    let other = until(&mut rt, "older change", |s| text(s).contains("+one"));
    assert_eq!(other.pane().scroll, 0);
    assert_eq!(other.pane().selections, [(0, 0)]);
}

#[test]
fn git_branch_switch_history_and_push_use_real_refs() {
    let (_dir, _state, root, mut rt) = repository();
    let upstream = tempfile::tempdir().unwrap();
    git(upstream.path(), &["init", "--bare", "-q"]);
    git(&root, &["remote", "add", "origin", upstream.path().to_str().unwrap()]);
    git(&root, &["push", "-qu", "origin", "main"]);
    git(&root, &["switch", "-qc", "topic"]);
    git(&root, &["add", "a.txt"]);
    git(&root, &["commit", "-qm", "topic work"]);
    git(&root, &["branch", "--set-upstream-to=origin/main", "topic"]);
    keys(&mut rt, "C-x v d");
    let status = until(&mut rt, "topic branch", |s| text(s).contains("Head:     topic") && text(s).contains("[ahead 1]"));
    assert!(text(&status).contains("Merge:    origin/main"));
    assert!(text(&status).contains("Unmerged into origin/main (1)") && !text(&status).contains("Unpulled"));
    keys(&mut rt, "l l");
    let history = until(&mut rt, "topic history", |s| text(s).contains("Commits in HEAD") && text(s).contains("topic work"));
    assert!(text(&history).contains("origin/main"), "{}", text(&history));
    goto(&mut rt, "topic work", 0);
    keys(&mut rt, "RET");
    until(&mut rt, "selected commit", |s| s.panes.iter().any(|p| p.text.to_string().contains("+ONE")));
    keys(&mut rt, "q q");
    until(&mut rt, "back to status", |s| text(s).starts_with("Git status"));
    keys(&mut rt, "P p");
    let preview =
        until(&mut rt, "push confirmation", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("origin:refs/heads/main")));
    assert_eq!(preview.minibuffer.unwrap().rows[0].text(0), "no");
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    until(&mut rt, "push completed", |s| s.echo == "Push branch finished");
    let shown = Command::new("git").current_dir(upstream.path()).args(["log", "-1", "--format=%s", "main"]).output().unwrap();
    assert!(shown.status.success());
    assert_eq!(shown.stdout, b"topic work\n");
    keys(&mut rt, "b b");
    until(&mut rt, "switch branch prompt", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Checkout")));
    type_text(&mut rt, "main");
    keys(&mut rt, "RET");
    until(&mut rt, "switched branch", |s| text(s).contains("Head:     main") && text(s).contains("Merge:    origin/main"));
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "one\n");
    keys(&mut rt, "F f");
    let prompt = until(&mut rt, "fast-forward confirmation", |s| {
        s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Fast-forward main to upstream origin/main @"))
    });
    assert_eq!(prompt.minibuffer.unwrap().rows[0].text(0), "no");
    keys(&mut rt, "RET");
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "one\n");
    keys(&mut rt, "F f");
    until(&mut rt, "fast-forward confirmation again", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Fast-forward main")));
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    until(&mut rt, "fast-forwarded branch", |s| s.echo == "Fast-forward finished");
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "ONE\n");
    keys(&mut rt, "M-< b c");
    until(&mut rt, "branch creation prompt", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("New branch at")));
    type_text(&mut rt, "follow-up");
    keys(&mut rt, "RET");
    until(&mut rt, "created branch", |s| text(s).contains("Head:     follow-up"));
    let head = Command::new("git").current_dir(&root).args(["symbolic-ref", "HEAD"]).output().unwrap();
    assert_eq!(head.stdout, b"refs/heads/follow-up\n");
    keys(&mut rt, "P p");
    until(&mut rt, "first push remote", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Publish branch to remote")));
    keys(&mut rt, "RET");
    let preview = until(&mut rt, "first push confirmation", |s| {
        s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("refs/heads/follow-up and set upstream"))
    });
    assert_eq!(preview.minibuffer.unwrap().rows[0].text(0), "no");
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    until(&mut rt, "first push finished", |s| s.echo == "Published branch and set upstream");
    let tracked = Command::new("git").current_dir(&root).args(["rev-parse", "--abbrev-ref", "@{upstream}"]).output().unwrap();
    assert!(tracked.status.success());
    assert_eq!(tracked.stdout, b"origin/follow-up\n");
    let tip = Command::new("git").current_dir(upstream.path()).args(["log", "-1", "--format=%s", "follow-up"]).output().unwrap();
    assert!(tip.status.success());
    assert_eq!(tip.stdout, b"topic work\n");
    let ref_id = |dir: &Path, name: &str| {
        let out = Command::new("git").current_dir(dir).args(["rev-parse", "--verify", name]).output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    };
    let original = ref_id(&root, "HEAD");
    let earlier = ref_id(&root, "HEAD^");
    git(&root, &["commit", "--amend", "-qm", "rewritten topic"]);
    let rewritten = ref_id(&root, "HEAD");
    keys(&mut rt, "P f");
    let prompt =
        until(&mut rt, "lease confirmation", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("only if it still points to")));
    assert!(prompt.minibuffer.unwrap().prompt.contains(&original[..8]));
    git(upstream.path(), &["update-ref", "refs/heads/follow-up", &earlier]);
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    until(&mut rt, "stale lease refused", |s| s.echo.contains("stale info"));
    assert_eq!(ref_id(upstream.path(), "refs/heads/follow-up"), earlier);
    git(upstream.path(), &["update-ref", "refs/heads/follow-up", &original]);
    keys(&mut rt, "P f");
    until(&mut rt, "second lease confirmation", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Replace published tip")));
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    until(&mut rt, "rewritten tip published", |s| s.echo == "Push with lease finished");
    assert_eq!(ref_id(upstream.path(), "refs/heads/follow-up"), rewritten);

    git(&root, &["tag", "-a", "release", "-m", "Reviewed release", "HEAD"]);
    let tag = ref_id(&root, "refs/tags/release");
    let choose_tag = |rt: &mut Runtime| {
        keys(rt, "P t");
        until(rt, "tag picker", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Push tag")));
        keys(rt, "RET");
        until(rt, "tag remote picker", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Push release to")));
        keys(rt, "RET");
        let prompt = until(rt, "tag push confirmation", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("refs/tags/release")));
        assert!(prompt.minibuffer.unwrap().prompt.contains(&tag[..8]));
    };
    choose_tag(&mut rt);
    let replacement = tempfile::tempdir().unwrap();
    git(replacement.path(), &["init", "--bare", "-q"]);
    git(&root, &["remote", "set-url", "--push", "origin", replacement.path().to_str().unwrap()]);
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    until(&mut rt, "changed tag destination rejected", |s| s.echo.contains("push destination changed"));
    assert!(git_out(replacement.path(), &["for-each-ref", "refs/tags/"]).is_empty());
    git(&root, &["remote", "set-url", "--push", "origin", upstream.path().to_str().unwrap()]);
    choose_tag(&mut rt);
    git(&root, &["tag", "-f", "release", "HEAD^"]);
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    until(&mut rt, "reviewed tag published", |s| s.echo == "Push tag finished");
    assert_eq!(ref_id(upstream.path(), "refs/tags/release"), tag);
}

#[test]
fn git_rebase_onto_upstream_can_continue_after_resolution() {
    let (_dir, _state, root, mut rt) = repository();
    let upstream = tempfile::tempdir().unwrap();
    let file = root.join("a.txt");
    git(upstream.path(), &["init", "--bare", "-q"]);
    git(&root, &["config", "user.name", "Editor Test"]);
    git(&root, &["config", "user.email", "test@example.test"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    git(&root, &["remote", "add", "origin", upstream.path().to_str().unwrap()]);
    git(&root, &["config", "merge.conflictStyle", "diff3"]);
    git(&root, &["switch", "-qc", "topic"]);
    git(&root, &["commit", "-qam", "topic edits"]);
    git(&root, &["switch", "-q", "main"]);
    std::fs::write(&file, "upstream edits\n").unwrap();
    git(&root, &["commit", "-qam", "upstream edits"]);
    git(&root, &["push", "-qu", "origin", "main"]);
    git(&root, &["switch", "-q", "topic"]);
    git(&root, &["branch", "--set-upstream-to=origin/main"]);
    keys(&mut rt, "C-x v d");
    until(&mut rt, "divergent topic", |s| text(s).contains("ahead 1, behind 1"));
    keys(&mut rt, "r u");
    let prompt = until(&mut rt, "rebase preview", |s| {
        s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Rebase topic onto upstream origin/main @"))
    });
    assert_eq!(prompt.minibuffer.unwrap().rows[0].text(0), "no");
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    let stopped =
        until(&mut rt, "rebase stopped at conflict", |s| text(s).contains("Rebasing topic onto") && text(s).contains("s stage resolution"));
    assert!(text(&stopped).contains("topic edits"), "the step it stopped at: {}", text(&stopped));
    keys(&mut rt, "r");
    let menu = pane_named(&rt.snapshot(), "*vc-menu*").unwrap();
    assert!(menu.contains("Abort") && menu.contains("Continue") && !menu.contains("Commit the merge"), "{menu}");
    keys(&mut rt, "C-g");
    assert!(pane_named(&rt.snapshot(), "*vc-menu*").is_none());
    let marked = std::fs::read_to_string(&file).unwrap();
    goto(&mut rt, "both modified", 0);
    keys(&mut rt, "s");
    until(&mut rt, "stage check prompt", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Mark a.txt resolved")));
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    until(&mut rt, "saved conflict markers refuse staging", |s| s.echo.contains("conflict marker check"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), marked);
    let unresolved = Command::new("git").current_dir(&root).args(["ls-files", "--unmerged", "--", "a.txt"]).output().unwrap();
    assert!(!unresolved.stdout.is_empty());
    goto(&mut rt, "both modified", 0);
    keys(&mut rt, "e");
    let panel = until(&mut rt, "conflict alternatives", |s| text(s).contains("Conflict at line 1"));
    assert!(text(&panel).contains("Common ancestor") && text(&panel).contains("upstream edits"));
    keys(&mut rt, "n 3");
    until(&mut rt, "resolved in buffer only", |s| s.echo == "Resolved in buffer; not saved or staged");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), marked);
    keys(&mut rt, "q C-x v d");
    until(&mut rt, "status with an unsaved resolution", |s| text(s).contains("Rebasing topic") && text(s).contains("Conflicts"));
    goto(&mut rt, "both modified", 0);
    keys(&mut rt, "s");
    until(&mut rt, "resolution confirmation", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Mark a.txt resolved")));
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    until(&mut rt, "unsaved resolution refused", |s| s.echo.contains("Save the resolved file buffer"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), marked);
    keys(&mut rt, "e q C-x C-s C-x v d");
    until(&mut rt, "saved resolution in status", |s| text(s).contains("Rebasing topic") && text(s).contains("Conflicts"));
    goto(&mut rt, "both modified", 0);
    keys(&mut rt, "s");
    until(&mut rt, "accept resolution", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Mark a.txt resolved")));
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    until(&mut rt, "resolution staged", |s| s.echo == "Stage resolution finished" && text(s).contains("r r continue"));
    keys(&mut rt, "r r");
    until(&mut rt, "rebase continued", |s| s.echo == "Continue rebase finished");
    until(&mut rt, "topic ahead, no longer diverged", |s| text(s).contains("Head:     topic") && text(s).contains("[ahead 1]"));
    assert_eq!(std::fs::read_to_string(file).unwrap(), "upstream edits\nONE\n");
    let log = Command::new("git").current_dir(&root).args(["log", "-2", "--format=%s"]).output().unwrap();
    assert_eq!(log.stdout, b"topic edits\nupstream edits\n");
}

#[test]
fn resolving_and_committing_a_git_merge_with_the_original_tree() {
    let (_dir, _state, root, mut rt) = repository();
    git(&root, &["config", "user.name", "Editor Test"]);
    git(&root, &["config", "user.email", "test@example.test"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    let file = root.join("a.txt");
    git(&root, &["branch", "topic"]);
    git(&root, &["add", "a.txt"]);
    git(&root, &["commit", "-qm", "ours"]);
    git(&root, &["switch", "-q", "topic"]);
    std::fs::write(&file, "theirs\n").unwrap();
    git(&root, &["add", "a.txt"]);
    git(&root, &["commit", "-qm", "theirs"]);
    git(&root, &["switch", "-q", "main"]);
    let merge = Command::new("git").current_dir(&root).args(["merge", "topic"]).output().unwrap();
    assert!(!merge.status.success());
    assert!(root.join(".git/MERGE_HEAD").is_file());
    keys(&mut rt, "C-x v d");
    until(&mut rt, "conflicted Git file", |s| text(s).contains("Conflicts"));
    std::fs::write(&file, "ONE\n").unwrap();
    goto(&mut rt, "both modified", 0);
    keys(&mut rt, "s");
    until(&mut rt, "resolution confirmation", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Mark a.txt resolved")));
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    until(&mut rt, "resolution recorded", |s| s.echo == "Stage resolution finished");
    keys(&mut rt, "U");
    until(&mut rt, "unstaged during merge", |s| s.echo == "Unstaged everything");
    assert!(root.join(".git/MERGE_HEAD").is_file());
    keys(&mut rt, "c c");
    until(&mut rt, "merge draft despite no patch", |s| s.pane().status.contains("vc-commit"));
    type_text(&mut rt, "Finish merge using ours");
    keys(&mut rt, "C-c C-c");
    until(&mut rt, "merge committed", |s| s.echo == "Committed staged changes");
    let parents = Command::new("git").current_dir(&root).args(["log", "-1", "--format=%P"]).output().unwrap();
    assert_eq!(String::from_utf8(parents.stdout).unwrap().split_whitespace().count(), 2);
}

#[test]
fn the_index_and_worktree_have_different_patches() {
    let (_dir, _state, root, mut rt) = repository();
    git(&root, &["add", "a.txt"]);
    std::fs::write(root.join("a.txt"), "worktree\n").unwrap();
    keys(&mut rt, "C-x v d");
    until(&mut rt, "both versions", |s| text(s).matches("a.txt").count() == 2);
    goto(&mut rt, "a.txt", 0);
    keys(&mut rt, "=");
    let s = until(&mut rt, "worktree patch", |s| s.panes.len() == 2);
    let patch = s.panes[1].text.to_string();
    assert!(patch.contains("-ONE\n+worktree"), "{patch}");
    goto(&mut rt, "a.txt", 1);
    keys(&mut rt, "=");
    let s = until(&mut rt, "index patch", |s| s.panes[1].text.to_string().contains("-one\n+ONE"));
    assert!(!s.panes[1].text.to_string().contains("+worktree"));
}

#[test]
fn staging_one_git_hunk_keeps_other_hunks_unstaged() {
    let (_dir, _state, root, mut rt) = repository();
    let original = (1..=35).map(|i| format!("line {i}\n")).collect::<String>();
    let file = root.join("a.txt");
    std::fs::write(&file, &original).unwrap();
    git(&root, &["add", "a.txt"]);
    git(&root, &["commit", "-qm", "multiple lines"]);
    let edited = original.replace("line 3\n", "first change\n").replace("line 30\n", "second change\n");
    std::fs::write(&file, &edited).unwrap();
    keys(&mut rt, "C-x v d");
    until(&mut rt, "the status", |s| text(s).contains("a.txt"));
    goto(&mut rt, "a.txt", 0);
    keys(&mut rt, "=");
    let s = until(&mut rt, "worktree patch", |s| s.panes.len() == 2);
    assert!(s.panes[1].text.to_string().starts_with("Index → working tree  a.txt"));
    keys(&mut rt, "C-x o n ?");
    let menu = pane_named(&rt.snapshot(), "*vc-menu*").unwrap();
    assert!(menu.contains("s  Stage hunk") && !menu.contains("Unstage"), "{menu}");
    keys(&mut rt, "C-g s");
    until(&mut rt, "the first hunk staged", |s| s.echo == "Staged");
    let index = || {
        let o = Command::new("git").current_dir(&root).args(["show", ":a.txt"]).output().unwrap();
        assert!(o.status.success());
        String::from_utf8(o.stdout).unwrap()
    };
    assert_eq!(index(), original.replace("line 3\n", "first change\n"));
    assert_eq!(std::fs::read_to_string(&file).unwrap(), edited);
    keys(&mut rt, "C-x o");
    until(&mut rt, "both status sides", |s| text(s).matches("a.txt").count() == 2);
    goto(&mut rt, "a.txt", 1);
    keys(&mut rt, "=");
    until(&mut rt, "the first hunk in the index", |s| s.panes.iter().any(|p| p.text.to_string().starts_with("HEAD → index  a.txt")));
    keys(&mut rt, "C-x o M-< n u");
    until(&mut rt, "the first hunk unstaged", |s| s.echo == "Unstaged");
    assert_eq!(index(), original);
    assert_eq!(std::fs::read_to_string(&file).unwrap(), edited);
}

#[test]
fn a_region_stages_and_unstages_only_the_selected_lines() {
    let (_dir, _state, root, mut rt) = repository();
    let file = root.join("a.txt");
    git(&root, &["config", "diff.suppressBlankEmpty", "true"]);
    git(&root, &["config", "diff.noprefix", "true"]);
    std::fs::write(&file, "before\n\nbase\nafter\n").unwrap();
    git(&root, &["add", "a.txt"]);
    git(&root, &["commit", "-qm", "text baseline"]);
    std::fs::write(&file, "before\n\nbase\nfirst addition\nsecond addition\nafter\n").unwrap();
    let index = || {
        let out = Command::new("git").current_dir(&root).args(["show", ":a.txt"]).output().unwrap();
        assert!(out.status.success());
        String::from_utf8(out.stdout).unwrap()
    };
    keys(&mut rt, "C-x v d");
    until(&mut rt, "changed file", |s| text(s).contains("a.txt"));
    goto(&mut rt, "a.txt", 0);
    keys(&mut rt, "TAB");
    until(&mut rt, "inline hunk", |s| text(s).contains("+second addition"));
    goto(&mut rt, "+first addition", 0);
    keys(&mut rt, "C-SPC C-n s");
    until(&mut rt, "line staged", |s| s.echo == "Staged");
    assert_eq!(index(), "before\n\nbase\nfirst addition\nafter\n");
    git(&root, &["add", "a.txt"]);
    keys(&mut rt, "M-< g");
    until(&mut rt, "staged file only", |s| text(s).contains("Staged changes") && !text(s).contains("Unstaged"));
    goto(&mut rt, "a.txt", 0);
    keys(&mut rt, "TAB");
    until(&mut rt, "staged patch", |s| text(s).contains("+second addition"));
    goto(&mut rt, "+first addition", 0);
    keys(&mut rt, "C-SPC C-n u");
    until(&mut rt, "line unstaged", |s| s.echo == "Unstaged");
    assert_eq!(index(), "before\n\nbase\nsecond addition\nafter\n");
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "before\n\nbase\nfirst addition\nsecond addition\nafter\n");
}

#[test]
fn inline_patches_can_be_opened_independently_and_staged() {
    let (_dir, _state, root, mut rt) = repository();
    std::fs::write(root.join("b.txt"), "ALSO TWO\n").unwrap();
    keys(&mut rt, "C-x v d");
    until(&mut rt, "both files", |s| text(s).contains("b.txt"));
    goto(&mut rt, "a.txt", 0);
    keys(&mut rt, "TAB");
    until(&mut rt, "first inline patch", |s| text(s).lines().any(|line| line == "+ONE"));
    keys(&mut rt, "n n TAB");
    let both = until(&mut rt, "second inline patch", |s| text(s).lines().any(|line| line == "+ALSO TWO"));
    assert!(text(&both).lines().any(|line| line == "+ONE"));
    assert_eq!(both.panes.len(), 1);
    keys(&mut rt, "n s");
    until(&mut rt, "second file's hunk staged", |s| s.echo == "Staged");
    let shown = Command::new("git").current_dir(&root).args(["show", ":b.txt"]).output().unwrap();
    assert!(shown.status.success());
    assert_eq!(shown.stdout, b"ALSO TWO\n");
    let status = until(&mut rt, "status after staging", |s| text(s).contains("▾ Staged changes"));
    assert!(text(&status).lines().any(|line| line == "+ONE"));
}

#[test]
fn staging_the_worktree_side_of_a_staged_rename() {
    let (_dir, _state, root, mut rt) = repository();
    std::fs::write(root.join("a.txt"), "one\n").unwrap();
    git(&root, &["mv", "b.txt", "renamed.txt"]);
    std::fs::write(root.join("renamed.txt"), "two, edited\n").unwrap();
    keys(&mut rt, "C-x v d");
    until(&mut rt, "both rename sides", |s| text(s).matches("renamed.txt").count() == 2);
    goto(&mut rt, "renamed.txt", 0);
    keys(&mut rt, "s");
    until(&mut rt, "staged rename destination", |s| s.echo == "Staged");
    let show = Command::new("git").current_dir(&root).args(["show", ":renamed.txt"]).output().unwrap();
    assert!(show.status.success());
    assert_eq!(show.stdout, b"two, edited\n");
}

#[test]
fn the_picker_discards_only_the_unstaged_side() {
    let (_dir, _state, root, mut rt) = repository();
    git(&root, &["add", "a.txt"]);
    std::fs::write(root.join("a.txt"), "worktree\n").unwrap();
    keys(&mut rt, "C-x v f");
    let s = until(&mut rt, "changed file sides", |s| s.minibuffer.as_ref().is_some_and(|m| m.rows.len() == 2));
    let rows = s.minibuffer.unwrap().rows;
    assert_eq!(rows[0].text(0), "a.txt");
    assert_eq!(rows[1].text(0), "a.txt");
    keys(&mut rt, "C-;");
    type_text(&mut rt, "revert");
    keys(&mut rt, "RET RET");
    until(&mut rt, "discard from the picker", |s| s.echo == "Reverted a.txt");
    let show = Command::new("git").current_dir(&root).args(["show", ":a.txt"]).output().unwrap();
    assert!(show.status.success());
    assert_eq!(show.stdout, b"ONE\n");
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "ONE\n");
}

#[test]
fn the_git_commit_buffer_commits_only_the_previewed_index() {
    let (_dir, _state, root, mut rt) = repository();
    git(&root, &["config", "user.name", "Editor Test"]);
    git(&root, &["config", "user.email", "test@example.test"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    git(&root, &["add", "a.txt"]);
    let gitlink = Command::new("git").current_dir(&root).args(["rev-parse", "HEAD"]).output().unwrap();
    let sha = String::from_utf8(gitlink.stdout).unwrap();
    git(&root, &["update-index", "--add", "--cacheinfo", &format!("160000,{},vendor/lib", sha.trim())]);
    git(&root, &["config", "diff.ignoreSubmodules", "all"]);
    std::fs::write(root.join("a.txt"), "unstaged\n").unwrap();
    keys(&mut rt, "C-x v d");
    until(&mut rt, "the status", |s| text(s).contains("a.txt"));
    keys(&mut rt, "c c");
    let s = until(&mut rt, "the commit draft", |s| s.pane().status.contains("vc-commit"));
    assert!(s.panes[1].text.to_string().contains("+ONE"));
    assert!(s.panes[1].text.to_string().contains(&format!("+Subproject commit {}", sha.trim())));
    assert!(!s.panes[1].text.to_string().contains("+unstaged"));
    type_text(&mut rt, "Update from the editor");
    keys(&mut rt, "C-c C-k c c");
    until(&mut rt, "the retained draft", |s| text(s) == "Update from the editor");
    git(&root, &["update-index", "--force-remove", "vendor/lib"]);
    keys(&mut rt, "C-c C-c");
    until(&mut rt, "the changed tree refused", |s| s.echo.contains("staged tree changed"));
    assert!(rt.snapshot().pane().status.contains("vc-commit"));
    git(&root, &["update-index", "--add", "--cacheinfo", &format!("160000,{},vendor/lib", sha.trim())]);
    git(&root, &["switch", "-qc", "same-tip"]);
    keys(&mut rt, "C-c C-c");
    until(&mut rt, "the switched branch refused", |s| s.echo.contains("staged tree changed"));
    assert_eq!(git_out(&root, &["rev-parse", "main"]), sha);
    assert_eq!(git_out(&root, &["rev-parse", "same-tip"]), sha);
    git(&root, &["switch", "-q", "main"]);
    keys(&mut rt, "C-c C-c");
    until(&mut rt, "the commit", |s| s.echo == "Committed staged changes");
    let show = Command::new("git").current_dir(&root).args(["show", "HEAD:a.txt"]).output().unwrap();
    assert!(show.status.success());
    assert_eq!(show.stdout, b"ONE\n");
    let entry = Command::new("git").current_dir(&root).args(["ls-tree", "HEAD", "vendor/lib"]).output().unwrap();
    assert!(String::from_utf8(entry.stdout).unwrap().contains(sha.trim()));
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "unstaged\n");
}

#[test]
fn reword_preserves_the_index_then_amend_commits_it() {
    let (_dir, _state, root, mut rt) = repository();
    git(&root, &["config", "user.name", "Editor Test"]);
    git(&root, &["config", "user.email", "test@example.test"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    git(&root, &["add", "a.txt"]);
    std::fs::write(root.join("a.txt"), "still in worktree\n").unwrap();
    let read_git = |args: &[&str]| {
        let out = Command::new("git").current_dir(&root).args(args).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    keys(&mut rt, "C-x v d");
    until(&mut rt, "the status", |s| text(s).contains("Staged"));
    let tree = read_git(&["rev-parse", "HEAD^{tree}"]);
    let index = read_git(&["write-tree"]);
    keys(&mut rt, "c w");
    let preview = until(&mut rt, "reword draft", |s| s.pane().status.contains("*vc-reword"));
    assert!(text(&preview).starts_with("start"));
    assert!(preview.panes[1].text.to_string().starts_with("Reword preview"));
    assert!(!preview.panes[1].text.to_string().contains("+ONE"));
    keys(&mut rt, "M-< C-k");
    type_text(&mut rt, "Better subject");
    keys(&mut rt, "C-c C-c");
    until(&mut rt, "reword finished", |s| s.echo == "Reworded HEAD; index unchanged");
    assert_eq!(read_git(&["log", "-1", "--format=%s"]), "Better subject\n");
    assert_eq!(read_git(&["rev-parse", "HEAD^{tree}"]), tree);
    assert_eq!(read_git(&["write-tree"]), index);
    keys(&mut rt, "c a");
    let preview = until(&mut rt, "amend draft", |s| s.pane().status.contains("*vc-amend"));
    assert!(text(&preview).starts_with("Better subject"));
    assert!(preview.panes[1].text.to_string().contains("+ONE"));
    keys(&mut rt, "C-c C-c");
    until(&mut rt, "amend finished", |s| s.echo == "Amended HEAD");
    assert_eq!(read_git(&["rev-parse", "HEAD^{tree}"]), index);
    assert_eq!(read_git(&["rev-list", "--count", "HEAD"]), "1\n");
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "still in worktree\n");
}

#[test]
fn jj_conflict_resolution_keeps_the_other_conflicted_file() {
    if !Command::new("jj").arg("version").output().is_ok_and(|out| out.status.success()) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let path = dir.path().join("a (cache).txt");
    let other = dir.path().join("z.txt");
    let jj = |args: &[&str]| {
        let out = Command::new("jj").args(args).current_dir(dir.path()).output().unwrap();
        assert!(out.status.success(), "jj {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    jj(&["git", "init", "--no-colocate"]);
    jj(&["config", "set", "--repo", "user.name", "Editor Test"]);
    jj(&["config", "set", "--repo", "user.email", "test@example.test"]);
    std::fs::write(&path, "base value\n").unwrap();
    std::fs::write(&other, "base other\n").unwrap();
    jj(&["describe", "-m", "Baseline"]);
    jj(&["bookmark", "set", "base"]);
    jj(&["new", "base", "-m", "First"]);
    std::fs::write(&path, "side one\n").unwrap();
    std::fs::write(&other, "side one other\n").unwrap();
    jj(&["bookmark", "set", "one"]);
    jj(&["new", "base", "-m", "Second"]);
    std::fs::write(&path, "side two\n").unwrap();
    std::fs::write(&other, "side two other\n").unwrap();
    jj(&["bookmark", "set", "two"]);
    jj(&["new", "one", "two", "-m", "Resolve this merge"]);
    let original = std::fs::read_to_string(&path).unwrap();
    let unchanged = std::fs::read_to_string(&other).unwrap();
    let mut rt = Runtime::open(&path, &state.path().join("journal"), "emacs").unwrap().0;
    keys(&mut rt, "C-x v d");
    let status = until(&mut rt, "recorded conflicted merge", |s| text(s).contains("Resolve this merge"));
    assert!(text(&status).contains("▾ Conflicts (2)"), "{}", text(&status));
    keys(&mut rt, "M-< n C");
    until(&mut rt, "choose conflict", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Recorded conflict")));
    type_text(&mut rt, "a (cache).txt");
    keys(&mut rt, "RET");
    let preview = until(&mut rt, "jj conflict preview", |s| text(s).contains("Recorded jj conflict"));
    assert!(text(&preview).contains("side one") && text(&preview).contains("side two") && text(&preview).contains("base value"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    keys(&mut rt, "?");
    let menu = pane_named(&rt.snapshot(), "*vc-menu*").unwrap();
    assert!(menu.contains("2  Take side 2"), "{menu}");
    keys(&mut rt, "C-g 2");
    let confirm = until(&mut rt, "confirm whole-file resolution", |s| {
        s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Replace all of a (cache).txt with side 2"))
    });
    assert_eq!(confirm.minibuffer.unwrap().rows[0].text(0), "no");
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    until(&mut rt, "resolution recorded", |s| s.echo.starts_with("Recorded Jujutsu operation"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "side two\n");
    assert_eq!(std::fs::read_to_string(&other).unwrap(), unchanged);
    assert_eq!(jj(&["--ignore-working-copy", "file", "show", "-r", "@", "root-file:\"a (cache).txt\""]), "side two\n");
    let conflicts = jj(&["--ignore-working-copy", "resolve", "--list"]);
    assert!(conflicts.contains("z.txt") && !conflicts.contains("a (cache).txt"), "{conflicts}");
}

#[test]
fn jj_split_and_squash_move_only_the_selected_hunk() {
    if !Command::new("jj").arg("version").output().is_ok_and(|out| out.status.success()) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let path = dir.path().join("a (cache).txt");
    let jj = |args: &[&str]| {
        let out = Command::new("jj").args(args).current_dir(dir.path()).output().unwrap();
        assert!(out.status.success(), "jj {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    let middle = (0..30).map(|i| format!("context {i}\n")).collect::<String>();
    let contents = |a: &str, b: &str, c: &str| format!("{a}\n{middle}{b}\n{middle}{c}\n");
    jj(&["git", "init", "--no-colocate"]);
    jj(&["config", "set", "--repo", "user.name", "Editor Test"]);
    jj(&["config", "set", "--repo", "user.email", "test@example.test"]);
    std::fs::write(&path, contents("old top", "old middle", "old bottom")).unwrap();
    jj(&["commit", "-m", "Baseline"]);
    std::fs::write(&path, contents("new top $left", "new middle", "new bottom")).unwrap();
    jj(&["describe", "-m", "Three edits"]);
    let recorded = |rev| jj(&["--ignore-working-copy", "file", "show", "-r", rev, "root-file:\"a (cache).txt\""]);
    let mut rt = Runtime::open(&path, &state.path().join("journal"), "emacs").unwrap().0;
    let hunks = |rt: &mut Runtime| {
        keys(rt, "M-< n F");
        until(rt, "recorded file list", |s| text(s).contains("Files of recorded change"));
        keys(rt, "M-< n h");
        until(rt, "recorded hunks", |s| text(s).contains("S split selected hunk"));
    };
    keys(&mut rt, "C-x v d");
    until(&mut rt, "Jujutsu graph", |s| text(s).contains("Three edits"));
    hunks(&mut rt);
    keys(&mut rt, "n S");
    until(&mut rt, "hunk split message", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Description for selected hunk")));
    type_text(&mut rt, "Top only");
    keys(&mut rt, "RET");
    until(&mut rt, "hunk split recorded", |s| s.echo.starts_with("Recorded Jujutsu operation"));
    assert_eq!(recorded("@-"), contents("new top $left", "old middle", "old bottom"));
    assert_eq!(recorded("@"), contents("new top $left", "new middle", "new bottom"));
    keys(&mut rt, "q q");
    until(&mut rt, "refreshed split graph", |s| text(s).contains("Top only"));
    hunks(&mut rt);
    keys(&mut rt, "n s");
    until(&mut rt, "hunk squash destination", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Squash")));
    type_text(&mut rt, "Top only");
    keys(&mut rt, "RET");
    until(&mut rt, "hunk squash recorded", |s| s.echo.starts_with("Recorded Jujutsu operation"));
    assert_eq!(recorded("@-"), contents("new top $left", "new middle", "old bottom"));
    assert_eq!(recorded("@"), contents("new top $left", "new middle", "new bottom"));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), contents("new top $left", "new middle", "new bottom"));
}

#[test]
fn jj_file_rewrites_are_scoped_and_reject_unreviewed_edits() {
    if !Command::new("jj").arg("version").output().is_ok_and(|out| out.status.success()) {
        eprintln!("skipping jj fixture: jj is not available");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let root = dir.path();
    let selected = root.join("a (cache).txt");
    let other = root.join("z.txt");
    let jj = |args: &[&str]| {
        let out = Command::new("jj").current_dir(root).args(args).output().unwrap();
        assert!(out.status.success(), "jj {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    jj(&["git", "init", "--no-colocate"]);
    jj(&["config", "set", "--repo", "user.name", "Editor Test"]);
    jj(&["config", "set", "--repo", "user.email", "test@example.test"]);
    std::fs::write(&selected, "cache base\n").unwrap();
    std::fs::write(&other, "other base\n").unwrap();
    jj(&["commit", "-m", "Base"]);
    std::fs::write(&selected, "cache edit\n").unwrap();
    std::fs::write(&other, "other edit\n").unwrap();
    jj(&["describe", "-m", "Combined work"]);
    let mut rt = Runtime::open(&selected, &state.path().join("journal"), "emacs").unwrap().0;
    keys(&mut rt, "C-x v =");
    let patch = until(&mut rt, "exact jj file path", |s| s.panes.iter().any(|p| p.text.to_string().contains("+cache edit")));
    assert!(!patch.panes.iter().any(|p| p.text.to_string().contains("+other edit")));
    keys(&mut rt, "C-x v l");
    until(&mut rt, "jj file history", |s| text(s).contains("File history · a (cache).txt") && text(s).contains("Combined work"));
    keys(&mut rt, "q C-x v d");
    until(&mut rt, "recorded change", |s| text(s).contains("Combined work"));
    keys(&mut rt, "M-< n F");
    until(&mut rt, "files of recorded change", |s| text(s).starts_with("Files of recorded change") && text(s).contains("z.txt"));
    keys(&mut rt, "n ?");
    let menu = pane_named(&rt.snapshot(), "*vc-menu*").unwrap();
    assert!(menu.starts_with("Jujutsu files") && menu.contains("RET Inspect recorded patch"), "{menu}");
    keys(&mut rt, "RET");
    let shown = until(&mut rt, "focused recorded file diff", |s| text(s).contains("+cache edit"));
    assert!(!shown.panes.iter().any(|p| p.text.to_string().contains("+other edit")));
    keys(&mut rt, "S");
    let menu = pane_named(&rt.snapshot(), "*vc-menu*").unwrap();
    assert!(menu.contains("Whole file · a (cache).txt") && menu.contains("Split this selection"), "{menu}");
    keys(&mut rt, "s");
    until(&mut rt, "split prompt", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Description of the part")));
    type_text(&mut rt, "Cache only");
    keys(&mut rt, "RET");
    until(&mut rt, "split recorded", |s| s.echo.starts_with("Recorded Jujutsu operation"));
    assert_eq!(jj(&["--ignore-working-copy", "file", "show", "-r", "@-", "root-file:\"a (cache).txt\""]), "cache edit\n");
    assert_eq!(jj(&["--ignore-working-copy", "file", "show", "-r", "@-", "z.txt"]), "other base\n");
    assert_eq!(jj(&["--ignore-working-copy", "file", "show", "-r", "@", "z.txt"]), "other edit\n");
    keys(&mut rt, "q");
    until(&mut rt, "refreshed graph", |s| text(s).contains("Cache only"));
    keys(&mut rt, "M-< n F");
    until(&mut rt, "remaining file", |s| {
        text(s).starts_with("Files of") && text(s).contains("z.txt") && !text(s).contains("a (cache).txt")
    });
    keys(&mut rt, "n s");
    until(&mut rt, "squash destination", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Squash z.txt into")));
    type_text(&mut rt, "Cache only");
    keys(&mut rt, "RET");
    until(&mut rt, "squash recorded", |s| s.echo.starts_with("Recorded Jujutsu operation"));
    assert_eq!(jj(&["--ignore-working-copy", "file", "show", "-r", "@-", "z.txt"]), "other edit\n");

    std::fs::write(&selected, "cache revised\n").unwrap();
    std::fs::write(&other, "other revised\n").unwrap();
    jj(&["describe", "-m", "Try restore"]);
    keys(&mut rt, "q g");
    until(&mut rt, "updated working-copy change", |s| text(s).contains("Try restore"));
    keys(&mut rt, "M-< n F");
    until(&mut rt, "restore file targets", |s| {
        text(s).starts_with("Files of") && text(s).contains("a (cache).txt") && text(s).contains("z.txt")
    });
    keys(&mut rt, "n k");
    until(&mut rt, "restore confirmation", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Discard edits to a (cache).txt")));
    std::fs::write(&selected, "cache unreviewed\n").unwrap();
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    until(&mut rt, "restore refuses unreviewed work", |s| s.echo.contains("Working copy changed"));
    assert_eq!(std::fs::read_to_string(&selected).unwrap(), "cache unreviewed\n");
    let commit = jj(&["--ignore-working-copy", "log", "--no-graph", "-r", "@", "-T", "commit_id"]);
    keys(&mut rt, "q");
    until(&mut rt, "graph shows the newly recorded commit", |s| text(s).contains(&commit[..8]));
    keys(&mut rt, "M-< n F");
    until(&mut rt, "fresh restore file targets", |s| {
        text(s).starts_with("Files of") && text(s).contains("a (cache).txt") && text(s).contains("z.txt")
    });
    keys(&mut rt, "n k");
    until(&mut rt, "fresh restore confirmation", |s| {
        s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Discard edits to a (cache).txt"))
    });
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    until(&mut rt, "restore recorded", |s| s.echo.starts_with("Recorded Jujutsu operation"));
    assert_eq!(std::fs::read_to_string(&selected).unwrap(), "cache edit\n");
    assert_eq!(std::fs::read_to_string(&other).unwrap(), "other revised\n");
}

#[test]
fn jj_browsing_does_not_snapshot_the_working_copy() {
    if !Command::new("jj").arg("version").output().is_ok_and(|out| out.status.success()) {
        eprintln!("skipping jj fixture: jj is not available");
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let file = root.join("a.txt");
    let jj = |args: &[&str]| {
        let out = Command::new("jj")
            .current_dir(&root)
            .args(["--config", "user.name='Demo'", "--config", "user.email='demo@example.test'"])
            .args(args)
            .output()
            .unwrap();
        assert!(out.status.success(), "jj {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    jj(&["git", "init"]);
    std::fs::write(&file, "recorded\n").unwrap();
    jj(&["describe", "-m", "Recorded work"]);
    let operation = || jj(&["--ignore-working-copy", "op", "log", "-n", "1", "--no-graph", "-T", "id"]);
    let before = operation();
    std::fs::write(&file, "not yet recorded\n").unwrap();
    let mut rt = Runtime::open(&file, &state.path().join("journal"), "emacs").unwrap().0;
    keys(&mut rt, "C-x v d");
    until(&mut rt, "the jj graph", |s| text(s).contains("Recorded work"));
    keys(&mut rt, "n RET");
    let s = until(&mut rt, "the recorded patch", |s| s.panes.iter().any(|p| p.text.to_string().contains("+recorded")));
    assert!(text(&s).contains("+recorded"));
    assert!(!s.panes.iter().any(|p| p.text.to_string().contains("+not yet recorded")));
    assert_eq!(operation(), before);
    keys(&mut rt, "q");
    keys(&mut rt, "/ C-a C-k");
    type_text(&mut rt, "none()");
    keys(&mut rt, "RET");
    let empty = until(&mut rt, "empty revset", |s| text(s).contains("No changes match this revset"));
    assert!(text(&empty).contains("Log · none() (0)"), "{}", text(&empty));
    assert_eq!(operation(), before);
    keys(&mut rt, "/ C-a C-k RET");
    let graph = until(&mut rt, "default revset again", |s| text(s).contains("Recorded work") && text(s).contains("\n@  "));
    let wc = text(&graph).find("@ ").unwrap();
    rt.eval(&format!("(move! (current-session) (lambda (_) {wc}))")).unwrap();
    keys(&mut rt, "G");
    let start = Instant::now();
    while rt.eval("(request-pending? (hash-table-ref (buffer-state (session-buffer (current-session))) 'slot))").unwrap() == "#t" {
        assert!(start.elapsed() < Duration::from_secs(20));
        rt.run_tasks(Duration::from_millis(5)).unwrap();
    }
    assert_ne!(operation(), before);
    keys(&mut rt, "=");
    until(&mut rt, "the new snapshot", |s| s.panes.iter().any(|p| p.text.to_string().contains("+not yet recorded")));
    jj(&["config", "set", "--repo", "user.name", "Demo"]);
    jj(&["config", "set", "--repo", "user.email", "demo@example.test"]);
    keys(&mut rt, "d");
    let draft = until(&mut rt, "description buffer", |s| s.pane().status.contains("*vc-describe"));
    assert_eq!(text(&draft), "Recorded work\n");
    assert!(draft.panes[1].text.to_string().contains("+not yet recorded"));
    keys(&mut rt, "M-< C-k");
    type_text(&mut rt, "Better description");
    keys(&mut rt, "RET RET");
    type_text(&mut rt, "Second paragraph from Techne.");
    keys(&mut rt, "C-c C-k d");
    until(&mut rt, "description draft kept", |s| s.pane().status.contains("*vc-describe") && text(s).contains("Second paragraph"));
    // jj snapshots again when accepting the prompt. The selected change
    // must follow the new working-copy commit, not create a divergent fork.
    std::fs::write(&file, "saved while describing\n").unwrap();
    keys(&mut rt, "C-c C-c");
    until(&mut rt, "recorded description", |s| s.echo.starts_with("Recorded Jujutsu operation"));
    assert!(
        jj(&["--ignore-working-copy", "log", "-r", "@ | @-", "--no-graph", "-T", "commit_id.short() ++ \" \" ++ description"])
            .contains("Better description"),
        "{} / {} / {}",
        jj(&["--ignore-working-copy", "log", "-r", "@", "--no-graph", "-T", "description"]),
        text(&rt.snapshot()),
        jj(&["--ignore-working-copy", "op", "log", "-n", "3"])
    );
    assert_eq!(
        jj(&["--ignore-working-copy", "log", "-r", "@", "--no-graph", "-T", "description"]),
        "Better description\n\nSecond paragraph from Techne.\n"
    );
    assert_eq!(jj(&["--ignore-working-copy", "file", "show", "-r", "@", "a.txt"]), "saved while describing\n");
    assert!(jj(&["--ignore-working-copy", "log", "-r", "divergent()", "--no-graph", "-T", "commit_id"]).is_empty());
    until(&mut rt, "renamed change", |s| text(s).contains("Better description"));
    keys(&mut rt, "X l");
    until(&mut rt, "operation log", |s| text(s).contains("Jujutsu operations"));
    keys(&mut rt, "n RET");
    until(&mut rt, "operation diff", |s| s.panes.iter().any(|p| p.text.to_string().starts_with("Jujutsu operation ·")));
    keys(&mut rt, "U");
    until(&mut rt, "revert prompt", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Revert local repo changes")));
    keys(&mut rt, "RET");
    until(&mut rt, "reverted operation", |s| s.echo.starts_with("Recorded Jujutsu operation"));
    assert_eq!(jj(&["--ignore-working-copy", "log", "-r", "@", "--no-graph", "-T", "description"]), "Recorded work\n");
    keys(&mut rt, "C-x v d");
    until(&mut rt, "status after revert", |s| text(s).starts_with("Jujutsu workspace"));
    let graph = rt.snapshot();
    let wc = text(&graph).find("@ ").unwrap();
    rt.eval(&format!("(move! (current-session) (lambda (_) {wc}))")).unwrap();
    keys(&mut rt, "o");
    let menu = pane_named(&rt.snapshot(), "*vc-menu*").unwrap();
    assert!(menu.contains("Inserted after it") && menu.contains("--no-edit"), "{menu}");
    keys(&mut rt, "o");
    until(&mut rt, "new working copy", |s| text(s).lines().any(|line| line.starts_with('@') && line.contains("(no description)")));
    std::fs::write(&file, "work to squash\n").unwrap();
    jj(&["describe", "-m", "Squash source"]);
    assert_eq!(jj(&["--ignore-working-copy", "log", "-r", "@-", "--no-graph", "-T", "description"]), "Recorded work\n");
    keys(&mut rt, "g");
    until(&mut rt, "refreshed graph", |s| text(s).contains("Squash source"));
    let graph = rt.snapshot();
    let wc = text(&graph).find("@ ").unwrap();
    rt.eval(&format!("(move! (current-session) (lambda (_) {wc}))")).unwrap();
    keys(&mut rt, "s s");
    until(&mut rt, "squashed change", |s| s.echo.starts_with("Recorded Jujutsu operation"));
    assert_eq!(jj(&["--ignore-working-copy", "file", "show", "-r", "@-", "a.txt"]), "work to squash\n");
    let operation_after_squash = operation();
    keys(&mut rt, "g");
    until(&mut rt, "graph after squash", |s| !text(s).contains("Squash source"));
    let graph = rt.snapshot();
    let wc = text(&graph).find("@ ").unwrap();
    rt.eval(&format!("(move! (current-session) (lambda (_) {wc}))")).unwrap();
    keys(&mut rt, "b b");
    until(&mut rt, "bookmark prompt", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Bookmark at")));
    type_text(&mut rt, "topic");
    keys(&mut rt, "RET");
    until(&mut rt, "bookmark recorded", |s| text(s).contains("topic"));
    assert_ne!(operation(), operation_after_squash);
    assert_eq!(jj(&["--ignore-working-copy", "log", "-r", "topic ~ @", "--no-graph", "-T", "commit_id"]), "");
    jj(&["describe", "-m", "Publish topic"]);
    let upstream = tempfile::tempdir().unwrap();
    git(upstream.path(), &["init", "--bare", "-q"]);
    jj(&["git", "remote", "add", "origin", upstream.path().to_str().unwrap()]);
    keys(&mut rt, "g");
    let graph = until(&mut rt, "published bookmark ready", |s| text(s).contains("Publish topic"));
    let wc = text(&graph).find("@ ").unwrap();
    rt.eval(&format!("(move! (current-session) (lambda (_) {wc}))")).unwrap();
    keys(&mut rt, "P - n p");
    until(&mut rt, "dry-run bookmark", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Publish bookmark")));
    keys(&mut rt, "RET");
    until(&mut rt, "dry-run remote", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Push to")));
    keys(&mut rt, "RET");
    until(&mut rt, "dry-run confirmation", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Preview push of topic")));
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    until(&mut rt, "dry-run completed", |s| s.echo == "No Jujutsu operation recorded");
    assert!(git_out(upstream.path(), &["for-each-ref", "refs/heads/topic"]).is_empty());
    keys(&mut rt, "P p");
    until(&mut rt, "bookmark to push", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Publish bookmark")));
    keys(&mut rt, "RET");
    until(&mut rt, "jj push remote", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Push to")));
    keys(&mut rt, "RET");
    until(&mut rt, "jj push confirmation", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Push topic")));
    type_text(&mut rt, "yes");
    keys(&mut rt, "RET");
    until(&mut rt, "bookmark pushed", |s| s.echo.starts_with("Recorded Jujutsu operation"));
    let shown = Command::new("git").current_dir(upstream.path()).args(["log", "-1", "--format=%s", "topic"]).output().unwrap();
    assert!(shown.status.success());
    assert_eq!(shown.stdout, b"Publish topic\n");

    jj(&["new", "-m", "Disposable candidate"]);
    jj(&["new", "root()", "-m", "Another working copy"]);
    keys(&mut rt, "g");
    let graph =
        until(&mut rt, "two candidate rows", |s| text(s).contains("Another working copy") && text(s).contains("Disposable candidate"));
    let pos = text(&graph).find("Disposable candidate").unwrap();
    rt.eval(&format!("(move! (current-session) (lambda (_) {pos}))")).unwrap();
    keys(&mut rt, "e");
    let graph = until(&mut rt, "edited change", |s| text(s).lines().any(|l| l.starts_with('@') && l.contains("Disposable candidate")));
    let pos = text(&graph).find("@  ").unwrap();
    rt.eval(&format!("(move! (current-session) (lambda (_) {pos}))")).unwrap();
    keys(&mut rt, "x");
    until(&mut rt, "abandon confirmation", |s| s.echo.starts_with("Abandon ") && s.echo.ends_with("(y or n) "));
    keys(&mut rt, "y");
    until(&mut rt, "abandoned change removed", |s| !text(s).contains("Disposable candidate"));
    assert!(
        jj(&[
            "--ignore-working-copy",
            "log",
            "-r",
            "::visible_heads() & description(exact:\"Disposable candidate\")",
            "--no-graph",
            "-T",
            "commit_id"
        ])
        .is_empty()
    );
}

/// Reloading the package while a refresh is in flight cancels it with the
/// old generation; the new one takes over the status buffer and refreshes
/// it by itself. Nothing is left running.
#[test]
fn reloading_while_a_refresh_is_in_flight() {
    let (_dir, _state, root, mut rt) = repository();
    let scopes = "(sort (map (lambda (s) (symbol->string (scope-name s))) (scope-children %root-scope)) string<?)";
    let builtins = rt.eval(scopes).unwrap();
    let path = lisp_dir().join("packages/vc.scm").display().to_string();
    keys(&mut rt, "C-x v d");
    until(&mut rt, "the status", |s| text(s).contains("a.txt"));
    std::fs::write(root.join("c.txt"), "new\n").unwrap();
    keys(&mut rt, "g");
    assert_eq!(rt.eval("(request-pending? (hash-table-ref (buffer-state (session-buffer (current-session))) 'slot))").unwrap(), "#t");
    rt.eval(&format!("(load-package 'vc {path:?})")).unwrap();
    assert_eq!(rt.eval("(package-generation (find-package 'vc))").unwrap(), "2");
    let s = until(&mut rt, "the new generation's refresh", |s| text(s).contains("c.txt"));
    assert!(text(&s).contains("▾ Untracked files (1)\n  ▸ c.txt\n▾ Unstaged changes (1)\n  ▸ a.txt"), "{}", text(&s));
    // It refreshes by itself: a file written later shows up.
    rt.eval("(set-option! 'vc-refresh-interval 20)").unwrap();
    std::fs::write(root.join("d.txt"), "later\n").unwrap();
    until(&mut rt, "the timer's refresh", |s| text(s).contains("d.txt"));
    rt.eval("(set-option! 'vc-refresh-interval 2000)").unwrap();
    // A last refresh may have started before the interval grew.
    let start = Instant::now();
    while rt.eval("(request-pending? (hash-table-ref (buffer-state (session-buffer (current-session))) 'slot))").unwrap() == "#t" {
        assert!(start.elapsed() < Duration::from_secs(20), "the last refresh never finished");
        rt.run_tasks(Duration::from_millis(5)).unwrap();
    }
    let live =
        "(length (filter (lambda (r) (guard (e (#t #f)) (not (task-done? r)))) (scope-resources (package-scope (find-package 'vc)))))";
    assert_eq!(rt.eval(live).unwrap(), "1", "the package runs its timer, nothing more");
    assert_eq!(rt.eval(scopes).unwrap(), builtins, "the old generation is gone");
    // Unloaded, its keys and commands go; the buffer stays, inert.
    rt.eval("(unload-package 'vc)").unwrap();
    assert_eq!(rt.eval("(memq 'vc-status (command-names))").unwrap(), "#f");
}

/// A refresh slower than the interval is left to finish: the timer does
/// not replace it, which would keep the view from ever refreshing.
#[test]
fn a_slow_refresh_is_not_replaced_by_the_timer() {
    let (_dir, _state, _root, mut rt) = repository();
    keys(&mut rt, "C-x v d");
    until(&mut rt, "the status", |s| text(s).contains("a.txt"));
    let slot = "(hash-table-ref (buffer-state (session-buffer (current-session))) 'slot)";
    rt.eval("(define slow-done #f)").unwrap();
    // Slower than the timer's sleep at the default interval, so it ticks.
    rt.eval(&format!("(request! {slot} (lambda () (sleep 2500)) (lambda (_) (set! slow-done #t)))")).unwrap();
    rt.eval("(set-option! 'vc-refresh-interval 1)").unwrap();
    let start = Instant::now();
    while rt.eval("slow-done").unwrap() != "#t" {
        assert!(start.elapsed() < Duration::from_secs(10), "the slow refresh was replaced");
        rt.run_tasks(Duration::from_millis(5)).unwrap();
    }
    rt.eval("(set-option! 'vc-refresh-interval #f)").unwrap();
}

/// A changed file's actions need nothing of the view: from the minibuffer,
/// with the view not shown, C-; offers them; reverting refreshes the views
/// of the repository.
#[test]
fn acting_on_changes_from_anywhere() {
    let (_dir, _state, root, mut rt) = repository();
    rt.eval("(set-option! 'vc-refresh-interval #f)").unwrap();
    keys(&mut rt, "C-x v d");
    until(&mut rt, "the status", |s| text(s).contains("a.txt"));
    keys(&mut rt, "C-x b RET");
    assert!(rt.snapshot().pane().status.contains("a.txt"));
    keys(&mut rt, "C-x v f");
    until(&mut rt, "the changes", |s| s.minibuffer.as_ref().is_some_and(|m| !m.rows.is_empty()));
    keys(&mut rt, "C-;");
    let s = rt.snapshot();
    let actions: Vec<String> = s.minibuffer.unwrap().rows.iter().map(|r| r.text(0)).collect();
    assert_eq!(actions, ["vc-visit", "vc-diff", "vc-revert", "vc-copy-path"]);
    type_text(&mut rt, "diff");
    keys(&mut rt, "RET");
    let s = until(&mut rt, "the diff", |s| s.panes.len() == 2);
    let diff = s.panes[1].text.to_string();
    assert!(diff.contains("-one") && diff.contains("+ONE"), "{diff}");
    // Revert, after asking: the file is back and the view no longer lists it.
    keys(&mut rt, "C-x 1 C-x v f");
    until(&mut rt, "the changes", |s| s.minibuffer.is_some());
    keys(&mut rt, "C-;");
    type_text(&mut rt, "revert");
    keys(&mut rt, "RET");
    assert_eq!(rt.snapshot().minibuffer.unwrap().prompt, "1/2 Discard unstaged edits to a.txt? ");
    keys(&mut rt, "RET");
    until(&mut rt, "the revert", |s| s.echo == "Reverted a.txt");
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "one\n");
    let name = format!("*vc {}*", root.display());
    let shown = |rt: &mut Runtime| rt.eval(&format!("(document-string (buffer-document (buffer-named {name:?})))")).unwrap();
    let start = Instant::now();
    while shown(&mut rt).contains("a.txt") {
        assert!(start.elapsed() < Duration::from_secs(20), "{}", shown(&mut rt));
        rt.run_tasks(Duration::from_millis(5)).unwrap();
    }
    assert!(shown(&mut rt).contains("No changes"), "{}", shown(&mut rt));
}

fn git_out(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").current_dir(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8(out.stdout).unwrap()
}

fn configure_git(root: &Path) {
    git(root, &["config", "user.name", "Editor Test"]);
    git(root, &["config", "user.email", "test@example.test"]);
    git(root, &["config", "commit.gpgsign", "false"]);
}

/// The commit menu's switches, the stash, fixups and an interactive
/// rebase planned in its buffer: the Magit loop from status, with keys.
#[test]
fn stash_fixup_and_an_interactive_rebase_from_the_menus() {
    let (_dir, _state, root, mut rt) = repository();
    configure_git(&root);
    keys(&mut rt, "C-x v d");
    until(&mut rt, "the status", |s| text(s).contains("▸ a.txt"));
    // A menu shows below and leaves with C-g; a switch shows it is set.
    keys(&mut rt, "c");
    let menu = pane_named(&rt.snapshot(), "*vc-menu*").unwrap();
    assert!(menu.contains("c  Commit") && menu.contains("e  Extend") && menu.contains("F  Instant fixup"), "{menu}");
    keys(&mut rt, "- a");
    assert!(pane_named(&rt.snapshot(), "*vc-menu*").unwrap().contains("(--all)"));
    keys(&mut rt, "C-g");
    assert_eq!(rt.snapshot().panes.len(), 1);
    // Stash the change, see it listed, pop it back.
    keys(&mut rt, "z z");
    until(&mut rt, "stash message", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Stash message")));
    keys(&mut rt, "RET");
    let stashed = until(&mut rt, "stash listed", |s| text(s).contains("▾ Stashes (1)"));
    assert!(text(&stashed).contains("stash@{0}") && !text(&stashed).contains("▸ a.txt"));
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "one\n");
    goto(&mut rt, "stash@{0}", 0);
    keys(&mut rt, "z k");
    until(&mut rt, "stash drop confirmation", |s| s.echo.contains("Drop stash@{0}?"));
    std::fs::write(root.join("a.txt"), "new stash\n").unwrap();
    git(&root, &["stash", "push", "-qm", "new stash"]);
    let newest = git_out(&root, &["rev-parse", "stash@{0}"]);
    keys(&mut rt, "y");
    until(&mut rt, "stale stash selector rejected", |s| s.echo.contains("Stash list changed"));
    assert_eq!(git_out(&root, &["rev-parse", "stash@{0}"]), newest);
    until(&mut rt, "both stashes shown", |s| text(s).contains("Stashes (2)"));
    git(&root, &["stash", "drop", "stash@{0}"]);
    keys(&mut rt, "g");
    until(&mut rt, "original stash shown again", |s| text(s).contains("Stashes (1)"));
    goto(&mut rt, "stash@{0}", 0);
    keys(&mut rt, "z p");
    until(&mut rt, "stash popped", |s| text(s).contains("▸ a.txt") && !text(s).contains("Stashes"));
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "ONE\n");
    // Commit with -a from the menu: nothing staged first.
    keys(&mut rt, "c - a c");
    until(&mut rt, "commit draft", |s| s.pane().status.contains("vc-commit"));
    type_text(&mut rt, "second");
    keys(&mut rt, "C-c C-c");
    until(&mut rt, "committed", |s| s.echo == "Committed staged changes" && s.panes.len() == 1);
    // A fixup of the first commit, folded in at once.
    std::fs::write(root.join("b.txt"), "two, fixed\n").unwrap();
    keys(&mut rt, "g");
    until(&mut rt, "b.txt changed", |s| text(s).contains("▸ b.txt"));
    goto(&mut rt, "▸ b.txt", 0);
    keys(&mut rt, "s");
    until(&mut rt, "b.txt staged", |s| text(s).contains("Staged changes"));
    goto(&mut rt, " start", 0);
    keys(&mut rt, "c F");
    until(&mut rt, "fixup folded in", |s| s.echo == "Autosquash finished");
    assert_eq!(git_out(&root, &["log", "--format=%s"]), "second\nstart\n");
    assert_eq!(git_out(&root, &["show", "HEAD^:b.txt"]), "two, fixed\n");
    // Two more commits, then a plan: squash one, reword the other.
    std::fs::write(root.join("b.txt"), "third\n").unwrap();
    git(&root, &["commit", "-qam", "third"]);
    std::fs::write(root.join("b.txt"), "fourth\n").unwrap();
    git(&root, &["commit", "-qam", "fourth"]);
    keys(&mut rt, "g");
    until(&mut rt, "four commits", |s| text(s).contains("Recent commits (4)"));
    goto(&mut rt, " second", 0);
    keys(&mut rt, "r i");
    let plan = until(&mut rt, "rebase plan", |s| text(s).starts_with("Rebase 3 commits"));
    assert!(text(&plan).contains("pick    ") && text(&plan).contains("fourth"));
    git(&root, &["commit", "--allow-empty", "-qm", "Arrived after planning"]);
    let new_tip = git_out(&root, &["rev-parse", "HEAD"]);
    keys(&mut rt, "C-c C-c");
    let after = until(&mut rt, "stale rebase rejected", |s| s.echo.contains("make a new rebase plan"));
    assert!(text(&after).contains("Rebase 3 commits"));
    assert_eq!(git_out(&root, &["rev-parse", "HEAD"]), new_tip);
    // Restore the original tip in this disposable fixture, then use the plan.
    git(&root, &["reset", "--hard", "HEAD^"]);
    goto(&mut rt, "third", 0);
    keys(&mut rt, "s");
    goto(&mut rt, "fourth", 0);
    keys(&mut rt, "r C-a C-k");
    type_text(&mut rt, "Fourth, renamed");
    keys(&mut rt, "RET");
    let planned = until(&mut rt, "planned", |s| text(s).contains("reword  "));
    assert!(text(&planned).contains("squash  "));
    keys(&mut rt, "C-c C-c");
    until(&mut rt, "rebased", |s| s.echo == "Interactive rebase finished");
    assert_eq!(git_out(&root, &["log", "--format=%s"]), "Fourth, renamed\nsecond\nstart\n");
    assert_eq!(git_out(&root, &["log", "-1", "--skip=1", "--format=%B"]), "second\n\nthird\n\n");
    assert_eq!(std::fs::read_to_string(root.join("b.txt")).unwrap(), "fourth\n");
}

/// k discards what is at point, after asking with one key: one hunk of a
/// file, leaving the other, and an untracked file.
#[test]
fn discarding_one_hunk_and_an_untracked_file() {
    let (_dir, _state, root, mut rt) = repository();
    let original = (1..=30).map(|i| format!("line {i}\n")).collect::<String>();
    std::fs::write(root.join("a.txt"), &original).unwrap();
    git(&root, &["commit", "-qam", "lines"]);
    let edited = original.replace("line 2\n", "line two\n").replace("line 29\n", "line twenty-nine\n");
    std::fs::write(root.join("a.txt"), &edited).unwrap();
    std::fs::write(root.join("new.txt"), "scratch\n").unwrap();
    keys(&mut rt, "C-x v d");
    until(&mut rt, "the status", |s| text(s).contains("▸ a.txt") && text(s).contains("new.txt"));
    goto(&mut rt, "▸ a.txt", 0);
    keys(&mut rt, "TAB");
    until(&mut rt, "hunks", |s| text(s).contains("+line twenty-nine"));
    goto(&mut rt, "+line two", 0);
    keys(&mut rt, "k");
    until(&mut rt, "one-key question", |s| s.echo.contains("Discard this hunk of a.txt? (y or n)"));
    keys(&mut rt, "y");
    until(&mut rt, "discarded", |s| s.echo == "Discarded");
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), original.replace("line 29\n", "line twenty-nine\n"));
    goto(&mut rt, "new.txt", 0);
    keys(&mut rt, "k");
    until(&mut rt, "delete question before replacement", |s| s.echo.contains("Delete untracked new.txt?"));
    let path = root.join("new.txt");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    std::fs::write(path.join("keep.txt"), "Keep this file\n").unwrap();
    keys(&mut rt, "y");
    until(&mut rt, "refused to delete the replacement", |s| s.echo.contains("File changed"));
    assert!(path.join("keep.txt").is_file());
    until(&mut rt, "replacement shown", |s| text(s).contains("new.txt/keep.txt"));
    std::fs::remove_dir_all(&path).unwrap();
    std::fs::write(&path, "new file\n").unwrap();
    keys(&mut rt, "g");
    until(&mut rt, "original path shown", |s| text(s).contains("new.txt") && !text(s).contains("keep.txt"));
    goto(&mut rt, "new.txt", 0);
    keys(&mut rt, "k");
    until(&mut rt, "delete question", |s| s.echo.contains("Delete untracked new.txt?"));
    keys(&mut rt, "y");
    until(&mut rt, "deleted", |s| !text(s).contains("new.txt"));
    assert!(!root.join("new.txt").exists());
}

/// The log is a graph of commits to act on: pick one onto the current
/// branch, revert it, and see the result in the status.
#[test]
fn cherry_pick_and_revert_from_the_log() {
    let (_dir, _state, root, mut rt) = repository();
    configure_git(&root);
    git(&root, &["switch", "-qc", "topic"]);
    git(&root, &["commit", "-qam", "topic change"]);
    git(&root, &["switch", "-q", "main"]);
    keys(&mut rt, "C-x v d");
    until(&mut rt, "status on main", |s| text(s).contains("Head:     main"));
    keys(&mut rt, "l b");
    let log = until(&mut rt, "graph of branches", |s| text(s).contains("Commits in --branches") && text(s).contains("topic change"));
    assert!(text(&log).contains("* "), "a graph: {}", text(&log));
    goto(&mut rt, "topic change", 0);
    keys(&mut rt, "A A");
    until(&mut rt, "picked", |s| s.echo.starts_with("Cherry-pick ") && s.echo.ends_with("finished"));
    assert_eq!(git_out(&root, &["log", "-1", "--format=%s", "main"]), "topic change\n");
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "ONE\n");
    keys(&mut rt, "l l");
    until(&mut rt, "main's log", |s| text(s).contains("Commits in HEAD"));
    goto(&mut rt, "topic change", 0);
    keys(&mut rt, "V V");
    until(&mut rt, "reverted", |s| s.echo.starts_with("Revert ") && s.echo.ends_with("finished"));
    assert!(git_out(&root, &["log", "-1", "--format=%s"]).starts_with("Revert \"topic change\""));
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "one\n");
    keys(&mut rt, "q");
    until(&mut rt, "back in status", |s| text(s).starts_with("Git status") && text(s).contains("Revert"));
}

/// Majutsu's loop, in the workspace view: open the working copy's file,
/// move one hunk into the parent, discard another, split the third into
/// its own change, undo; and rebase a marked change onto another.
#[test]
fn jj_hunks_from_the_workspace_and_marked_rebase() {
    if !Command::new("jj").arg("version").output().is_ok_and(|out| out.status.success()) {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let state = tempfile::tempdir().unwrap();
    let root = dir.path();
    let path = root.join("c.txt");
    let jj = |args: &[&str]| {
        let out = Command::new("jj").current_dir(root).args(args).output().unwrap();
        assert!(out.status.success(), "jj {args:?}: {}", String::from_utf8_lossy(&out.stderr));
        String::from_utf8(out.stdout).unwrap()
    };
    let lines = |edits: &[(usize, &str)]| {
        (1..=30).map(|i| edits.iter().find(|e| e.0 == i).map_or(format!("{i}\n"), |e| format!("{}\n", e.1))).collect::<String>()
    };
    jj(&["git", "init"]);
    jj(&["config", "set", "--repo", "user.name", "Editor Test"]);
    jj(&["config", "set", "--repo", "user.email", "test@example.test"]);
    std::fs::write(&path, lines(&[])).unwrap();
    jj(&["commit", "-m", "Base"]);
    std::fs::write(&path, lines(&[(2, "TWO"), (15, "FIFTEEN"), (28, "TWENTY-EIGHT")])).unwrap();
    jj(&["describe", "-m", "Three hunks"]);
    let show = |rev: &str| jj(&["--ignore-working-copy", "file", "show", "-r", rev, "c.txt"]);
    let mut rt = Runtime::open(&path, &state.path().join("journal"), "emacs").unwrap().0;
    keys(&mut rt, "C-x v d");
    until(&mut rt, "working copy changes", |s| text(s).contains("Working copy changes (1)"));
    goto(&mut rt, "▸ c.txt", 0);
    keys(&mut rt, "TAB");
    let hunks = until(&mut rt, "hunks", |s| text(s).contains("▾ c.txt") && text(s).contains("+TWENTY-EIGHT"));
    let inline = text(&hunks);
    assert!(inline.lines().any(|line| line.starts_with("@@ ")), "hunk headers start at the left edge");
    assert!(inline.lines().any(|line| line == "+TWENTY-EIGHT"), "code starts right after the diff sign: {inline}");
    assert!(inline.lines().any(|line| line == " 29"), "context and changed code share a column");
    let at = inline.find("+TWENTY-EIGHT").unwrap();
    assert!(hunks.pane().layers.iter().any(|h| h.from <= at && at < h.to && hunks.style(h.style).bg.is_some()));
    keys(&mut rt, "M-< n RET");
    until(&mut rt, "focused Jujutsu change", |s| text(s).contains("F changed files") && text(s).contains("+TWO"));
    keys(&mut rt, "F");
    until(&mut rt, "files of the focused change", |s| text(s).starts_with("Files of recorded change"));
    keys(&mut rt, "n RET");
    until(&mut rt, "focused Jujutsu file", |s| text(s).contains("C-SPC selects lines") && text(s).contains("+TWO"));
    keys(&mut rt, "n s");
    let menu = pane_named(&rt.snapshot(), "*vc-menu*").unwrap();
    assert!(menu.contains("Hunk at point · c.txt") && menu.contains("Move into parent"), "{menu}");
    keys(&mut rt, "s");
    until(&mut rt, "hunk squashed", |s| s.echo.starts_with("Recorded Jujutsu operation"));
    assert_eq!(show("@-"), lines(&[(2, "TWO")]));
    keys(&mut rt, "q q");
    until(&mut rt, "refreshed graph", |s| text(s).contains("+TWENTY-EIGHT") && !text(s).contains("+TWO"));
    // The inline patch stays open across the rewrite, on the change's new commit.
    goto(&mut rt, "+TWENTY-EIGHT", 0);
    keys(&mut rt, "R");
    until(&mut rt, "restore question", |s| s.echo.ends_with("(y or n) "));
    keys(&mut rt, "y");
    until(&mut rt, "hunk restored", |s| s.echo.starts_with("Recorded Jujutsu operation") && !text(s).contains("+TWENTY-EIGHT"));
    assert_eq!(show("@"), lines(&[(2, "TWO"), (15, "FIFTEEN")]));
    assert_eq!(std::fs::read_to_string(&path).unwrap(), lines(&[(2, "TWO"), (15, "FIFTEEN")]));
    goto(&mut rt, "+FIFTEEN", 0);
    keys(&mut rt, "S");
    let menu = pane_named(&rt.snapshot(), "*vc-menu*").unwrap();
    assert!(menu.contains("Hunk at point · c.txt") && menu.contains("Split this selection"), "{menu}");
    keys(&mut rt, "s");
    until(&mut rt, "split description", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Description of the part")));
    type_text(&mut rt, "Fifteen alone");
    keys(&mut rt, "RET");
    until(&mut rt, "split", |s| text(s).contains("Fifteen alone"));
    assert_eq!(show("@-"), lines(&[(2, "TWO"), (15, "FIFTEEN")]));
    keys(&mut rt, "u");
    until(&mut rt, "undone", |s| s.echo == "Undone · U redoes" && !text(s).contains("Fifteen alone"));
    // On a change, S reads the files to split out.
    keys(&mut rt, "M-< n S s");
    until(&mut rt, "file to split", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Split out file")));
    type_text(&mut rt, "c.txt");
    keys(&mut rt, "RET");
    until(&mut rt, "split description", |s| s.minibuffer.as_ref().is_some_and(|m| m.prompt.contains("Description of the part")));
    type_text(&mut rt, "Whole file");
    keys(&mut rt, "RET");
    until(&mut rt, "file split", |s| text(s).contains("Whole file"));
    assert_eq!(show("@-"), lines(&[(2, "TWO"), (15, "FIFTEEN")]));
    keys(&mut rt, "u");
    until(&mut rt, "split undone", |s| s.echo == "Undone · U redoes" && !text(s).contains("Whole file"));
    // Two siblings: mark one, rebase it onto the other.
    jj(&["new", "-m", "Left", "@-"]);
    jj(&["new", "-m", "Right", "@-"]);
    keys(&mut rt, "g");
    let graph = until(&mut rt, "siblings", |s| text(s).contains("Left") && text(s).contains("Right"));
    assert!(text(&graph).contains("▾ Log"));
    goto(&mut rt, "Left  ", 0);
    keys(&mut rt, "m");
    until(&mut rt, "marked", |s| s.echo.starts_with("1 marked"));
    goto(&mut rt, "Right  ", 0);
    keys(&mut rt, "r m");
    until(&mut rt, "rebased", |s| s.echo.starts_with("Recorded Jujutsu operation"));
    assert_eq!(jj(&["--ignore-working-copy", "log", "--no-graph", "-r", "description(exact:\"Left\n\")-", "-T", "description"]), "Right\n");
}

/// A region across hunks of one file stages the changed lines it covers
/// in each, as Magit does, and only those.
#[test]
fn a_region_across_hunks_stages_the_lines_it_covers() {
    let (_dir, _state, root, mut rt) = repository();
    let original = (1..=30).map(|i| format!("line {i}\n")).collect::<String>();
    std::fs::write(root.join("a.txt"), &original).unwrap();
    git(&root, &["commit", "-qam", "lines"]);
    let edited = original.replace("line 3\n", "changed 3\n").replace("line 28\n", "changed 28\n").replace("line 29\n", "changed 29\n");
    std::fs::write(root.join("a.txt"), &edited).unwrap();
    keys(&mut rt, "C-x v d");
    until(&mut rt, "the status", |s| text(s).contains("▸ a.txt"));
    goto(&mut rt, "▸ a.txt", 0);
    keys(&mut rt, "TAB");
    let shown = until(&mut rt, "two hunks", |s| text(s).contains("+changed 29"));
    let from = text(&shown).find("\n-line 3").unwrap() + 1;
    let to = text(&shown).find("\n+changed 28").unwrap() + "\n+changed 28".len();
    rt.eval(&format!("(move! (current-session) (lambda (_) {from}))")).unwrap();
    keys(&mut rt, "C-SPC");
    rt.eval(&format!("(move! (current-session) (lambda (_) {to}))")).unwrap();
    keys(&mut rt, "s");
    until(&mut rt, "staged", |s| s.echo == "Staged");
    let index = git_out(&root, &["show", ":a.txt"]);
    // The region holds both deletions of the second hunk, one addition.
    assert_eq!(index, original.replace("line 3\n", "changed 3\n").replace("line 28\nline 29\n", "changed 28\n"));
}
