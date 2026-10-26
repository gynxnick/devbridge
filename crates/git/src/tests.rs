use super::*;

fn git() -> Git {
    Git::locate(None).expect("git must be installed to run these tests")
}

/// Runs git with a throwaway identity (tests must not depend on user config).
fn sh(dir: &Path, args: &[&str]) {
    let st = Command::new("git")
        .args(["-c", "user.name=Test", "-c", "user.email=t@example.com", "-c", "init.defaultBranch=main"])
        .args(args)
        .current_dir(dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap();
    assert!(st.success(), "git {args:?}");
}

fn identity(dir: &Path) {
    sh(dir, &["config", "user.name", "Test"]);
    sh(dir, &["config", "user.email", "t@example.com"]);
}

/// origin (bare) + clones a and b, with one initial commit pushed.
fn three_repos() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let t = tempfile::tempdir().unwrap();
    let root = canonicalize(t.path()).unwrap();
    sh(&root, &["init", "-q", "--bare", "origin.git"]);
    sh(&root, &["clone", "-q", "origin.git", "a"]);
    let a = root.join("a");
    identity(&a);
    std::fs::write(a.join("f.txt"), "one\n").unwrap();
    sh(&a, &["add", "."]);
    sh(&a, &["commit", "-qm", "c1"]);
    sh(&a, &["push", "-q", "origin", "HEAD"]);
    sh(&root, &["clone", "-q", "origin.git", "b"]);
    let b = root.join("b");
    identity(&b);
    (t, a, b)
}

#[test]
fn parses_versions() {
    assert_eq!(parse_version("git version 2.54.0 (Apple Git-157)").as_deref(), Some("2.54.0"));
    assert_eq!(parse_version("git version 2.47.1.windows.1").as_deref(), Some("2.47.1.windows.1"));
}

#[test]
fn discover_init_refuses_nesting() {
    let g = git();
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(g.discover(dir.path()).unwrap(), None);
    let info = g.init(dir.path()).unwrap();
    assert!(info.is_toplevel && info.head.is_none());
    let sub = dir.path().join("sub");
    std::fs::create_dir(&sub).unwrap();
    assert!(!g.discover(&sub).unwrap().unwrap().is_toplevel);
    assert!(g.init(&sub).is_err());
}

#[test]
fn stage_commit_log_status_on_fresh_repo() {
    let g = git();
    let d = tempfile::tempdir().unwrap();
    g.init(d.path()).unwrap();
    identity(d.path());
    assert!(g.log(d.path(), 5).unwrap().is_empty());
    std::fs::write(d.path().join("a.txt"), "x").unwrap();
    let st = g.status(d.path()).unwrap();
    assert_eq!(st.entries[0].kind, EntryKind::Untracked);
    g.stage(d.path(), &["a.txt".into()]).unwrap();
    assert!(g.status(d.path()).unwrap().entries[0].staged());
    g.unstage(d.path(), &["a.txt".into()]).unwrap();
    assert_eq!(g.status(d.path()).unwrap().entries[0].kind, EntryKind::Untracked);
    g.stage(d.path(), &["a.txt".into()]).unwrap();
    assert!(matches!(g.commit(d.path(), "  "), Err(GitError::Refused(_))));
    let h = g.commit(d.path(), "first: with \"quotes\" and\n\nbody paragraph").unwrap();
    let log = g.log(d.path(), 5).unwrap();
    assert_eq!(log[0].hash, h);
    assert_eq!(log[0].subject, "first: with \"quotes\" and");
    assert!(g.status(d.path()).unwrap().is_clean());
    std::fs::write(d.path().join("a.txt"), "y").unwrap();
    g.unstage(d.path(), &["a.txt".into()]).unwrap(); // no-op on unstaged file
    assert_eq!(std::fs::read_to_string(d.path().join("a.txt")).unwrap(), "y", "unstage never touches the worktree");
}

#[test]
fn switch_requires_confirmation_when_dirty() {
    let g = git();
    let (_t, a, _b) = three_repos();
    g.switch(&a, "feature", true, false).unwrap();
    assert_eq!(g.status(&a).unwrap().branch.as_deref(), Some("feature"));
    std::fs::write(a.join("f.txt"), "dirty\n").unwrap();
    assert!(matches!(g.switch(&a, "main", false, false), Err(GitError::NeedsConfirmation { .. })));
    g.switch(&a, "main", false, true).unwrap();
    assert_eq!(std::fs::read_to_string(a.join("f.txt")).unwrap(), "dirty\n", "uncommitted work carried over intact");
    let br = g.branches(&a).unwrap();
    assert!(br.iter().any(|b| b.name == "feature"));
    assert!(br.iter().find(|b| b.name == "main").unwrap().current);
}

#[test]
fn pull_push_and_remotes() {
    let g = git();
    let (_t, a, b) = three_repos();
    assert_eq!(g.remotes(&a).unwrap()[0].name, "origin");
    std::fs::write(a.join("g.txt"), "new\n").unwrap();
    g.stage(&a, &["g.txt".into()]).unwrap();
    g.commit(&a, "c2").unwrap();
    g.push(&a, None).unwrap();

    g.fetch(&b).unwrap();
    assert_eq!(g.status(&b).unwrap().behind, 1);
    g.pull(&b, false).unwrap();
    assert!(b.join("g.txt").exists());

    std::fs::write(b.join("f.txt"), "local edit\n").unwrap();
    assert!(matches!(g.pull(&b, false), Err(GitError::NeedsConfirmation { .. })));

    g.remote_add(&a, "backup", "https://user:ghp_secrettokensecrettoken12@example.com/x.git").unwrap();
    let r = g.remotes(&a).unwrap();
    let backup = r.iter().find(|r| r.name == "backup").unwrap();
    assert_eq!(backup.url, "https://***@example.com/x.git", "credentials never surface");
    assert!(g.remote_add(&a, "--upload-pack=evil", "x").is_err());
    assert!(g.remote_add(&a, "x", "ext::sh -c touch% /tmp/pwned").is_err());
    g.remote_remove(&a, "backup").unwrap();
}

#[test]
fn catch_up_after_files_were_synced() {
    let g = git();
    let (_t, a, b) = three_repos();
    // Device A commits and pushes.
    std::fs::write(a.join("f.txt"), "two\n").unwrap();
    std::fs::write(a.join("g.txt"), "new\n").unwrap();
    g.stage(&a, &["f.txt".into(), "g.txt".into()]).unwrap();
    g.commit(&a, "c2").unwrap();
    g.push(&a, None).unwrap();
    // DevBridge synced the same files to B, then B fetched.
    std::fs::write(b.join("f.txt"), "two\n").unwrap();
    std::fs::write(b.join("g.txt"), "new\n").unwrap();
    g.fetch(&b).unwrap();
    // A plain pull refuses (this is the problem catch_up solves).
    assert!(g.pull(&b, true).is_err());

    let r = g.catch_up(&b).unwrap();
    assert_eq!(r.files, 2);
    let st = g.status(&b).unwrap();
    assert!(st.is_clean(), "{st:?}");
    assert_eq!(st.behind, 0);
    assert_eq!(std::fs::read_to_string(b.join("f.txt")).unwrap(), "two\n");
    let stashes = g.run(&b, &["stash", "list"]).unwrap();
    assert!(stashes.trim().is_empty(), "safety stash dropped after verification");
}

/// Windows runners (and many Windows users) have core.autocrlf=true: a merge
/// would rewrite the synced LF files with CRLF. Catch-up must not write files.
#[test]
fn catch_up_never_rewrites_files_even_with_autocrlf() {
    let g = git();
    let (_t, a, b) = three_repos();
    std::fs::write(a.join("f.txt"), "line one\nline two\n").unwrap();
    g.stage(&a, &["f.txt".into()]).unwrap();
    g.commit(&a, "c2").unwrap();
    g.push(&a, None).unwrap();
    sh(&b, &["config", "core.autocrlf", "true"]);
    std::fs::write(b.join("f.txt"), "line one\nline two\n").unwrap();
    g.fetch(&b).unwrap();
    g.catch_up(&b).unwrap();
    assert_eq!(
        std::fs::read(b.join("f.txt")).unwrap(),
        b"line one\nline two\n",
        "bytes untouched (no CRLF conversion)"
    );
    let st = g.status(&b).unwrap();
    assert!(st.is_clean(), "{st:?}");
    assert_eq!(st.behind, 0);
}

#[test]
fn catch_up_refuses_when_content_differs() {
    let g = git();
    let (_t, a, b) = three_repos();
    std::fs::write(a.join("f.txt"), "two\n").unwrap();
    g.stage(&a, &["f.txt".into()]).unwrap();
    g.commit(&a, "c2").unwrap();
    g.push(&a, None).unwrap();
    std::fs::write(b.join("f.txt"), "something else\n").unwrap();
    g.fetch(&b).unwrap();
    assert!(matches!(g.catch_up(&b), Err(GitError::Refused(_))));
    assert_eq!(std::fs::read_to_string(b.join("f.txt")).unwrap(), "something else\n", "nothing changed");

    // Unrelated dirty file also blocks it.
    std::fs::write(b.join("f.txt"), "two\n").unwrap();
    std::fs::write(b.join("unrelated.txt"), "wip\n").unwrap();
    assert!(matches!(g.catch_up(&b), Err(GitError::Refused(_))));
    assert!(b.join("unrelated.txt").exists());
}
