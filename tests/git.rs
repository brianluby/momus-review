//! Integration tests for the git adapter: discovery + guarded reads against a
//! real temp repository. Living under `tests/` also gives `momus scan` a
//! related-test context for the `testGap` dimension (stem `git` ↔
//! `src/adapters/git.rs`).

use std::fs;
use std::path::Path;
use std::process::Command;

use momus_review::adapters::exclude::Exclude;
use momus_review::adapters::git;

fn run_git(repo: &Path, args: &[&str]) {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("git runs");
    assert!(
        output.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn write(repo: &Path, rel: &str, content: &str) {
    let path = repo.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, content).unwrap();
}

/// A temp repo with a configured identity and a committed baseline.
fn fixture_repo() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().to_path_buf();
    run_git(&repo, &["init", "-q"]);
    run_git(&repo, &["config", "user.email", "t@t.co"]);
    run_git(&repo, &["config", "user.name", "t"]);
    // Isolate the fixture from developer-wide signing/hook config.
    run_git(&repo, &["config", "commit.gpgsign", "false"]);
    run_git(&repo, &["config", "core.hooksPath", "nonexistent-hooks"]);
    (dir, repo)
}

#[test]
fn repository_files_discovers_only_source_files() {
    let (_dir, repo) = fixture_repo();
    write(&repo, "src/lib.rs", "pub fn f() -> i32 { 1 }\n");
    write(&repo, "tests/lib_test.rs", "#[test]\nfn it_works() {}\n");
    write(&repo, "README.md", "# hi\n");
    write(&repo, "notes.py", "x = 1\n");
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "seed"]);

    let files = git::repository_files(std::slice::from_ref(&repo), &Exclude::default()).unwrap();
    let mut paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    paths.sort();

    // Test files are returned too (they become test-gap context); non-source
    // files are silently skipped.
    assert_eq!(paths, vec!["src/lib.rs", "tests/lib_test.rs"]);
}

#[test]
fn exclude_filters_discovered_paths() {
    let (_dir, repo) = fixture_repo();
    write(&repo, "src/lib.rs", "pub fn f() -> i32 { 1 }\n");
    write(&repo, "frontend/src/assets/three.js", "var x = 1;\n");
    write(&repo, "frontend/src/app/app.js", "console.log('hi');\n");
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "seed"]);

    let exclude = Exclude::new(&["frontend/src/assets/**".to_string()]).unwrap();
    let files = git::repository_files(std::slice::from_ref(&repo), &exclude).unwrap();
    let mut paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    paths.sort();

    // The vendored subtree is skipped; the app file and source remain.
    assert_eq!(paths, vec!["frontend/src/app/app.js", "src/lib.rs"]);
}

#[test]
fn changed_files_finds_tracked_diff_and_untracked_source() {
    let (_dir, repo) = fixture_repo();
    write(&repo, "src/lib.rs", "pub fn f() -> i32 { 1 }\n");
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "seed"]);

    // Tracked modification + untracked source file.
    write(&repo, "src/lib.rs", "pub fn f() -> i32 { 2 }\n");
    write(&repo, "src/extra.rs", "pub fn g() -> i32 { 3 }\n");

    let files = git::changed_files(std::slice::from_ref(&repo), &Exclude::default()).unwrap();
    let mut paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    paths.sort();
    assert_eq!(paths, vec!["src/extra.rs", "src/lib.rs"]);

    // The tracked file carries a real unified diff; the untracked file is
    // rendered as an all-additions patch.
    let tracked = files.iter().find(|f| f.path == "src/lib.rs").unwrap();
    assert!(tracked.patch.contains("@@"));
    assert!(tracked.patch.contains("pub fn f() -> i32 { 2 }"));

    let untracked = files.iter().find(|f| f.path == "src/extra.rs").unwrap();
    assert!(untracked.patch.starts_with("@@ -0,0 +1,"));
    assert!(untracked.patch.contains("+pub fn g() -> i32 { 3 }"));
}

#[test]
fn multi_scope_unions_and_dedupes() {
    let (_dir, repo) = fixture_repo();
    write(&repo, "server/app.js", "console.log('a');\n");
    write(&repo, "server/routes/search.js", "console.log('s');\n");
    write(&repo, "frontend/app.js", "console.log('f');\n");
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "seed"]);

    // Overlapping scopes: "server" plus "server/routes".
    let scopes = vec![repo.join("server"), repo.join("server").join("routes")];
    let files = git::repository_files(&scopes, &Exclude::default()).unwrap();
    let mut paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    paths.sort();

    // Union of both scopes, deduped; frontend/app.js is outside the scopes.
    assert_eq!(paths, vec!["server/app.js", "server/routes/search.js"]);
}