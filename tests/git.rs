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
    write(&repo, "data.csv", "a,b\n1,2\n");
    write(&repo, "src/worker.go", "package main\n");
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "seed"]);

    let files = git::repository_files(std::slice::from_ref(&repo), &Exclude::default()).unwrap();
    let mut paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    paths.sort();

    // Test files are returned too (they become test-gap context); non-source
    // files are silently skipped, and every supported language is discovered.
    assert_eq!(
        paths,
        vec!["src/lib.rs", "src/worker.go", "tests/lib_test.rs"]
    );
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
#[test]
fn renamed_file_diffs_against_its_old_path() {
    let (_dir, repo) = fixture_repo();
    write(
        &repo,
        "src/old.rs",
        "fn a() {}\nfn b() {}\nfn c() {}\nfn d() {}\n",
    );
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "seed"]);

    run_git(&repo, &["mv", "src/old.rs", "src/new.rs"]);
    write(
        &repo,
        "src/new.rs",
        "fn a() {}\nfn b() {}\nfn c() {}\nfn d() {}\nfn z() {}\n",
    );

    let files = git::changed_files(std::slice::from_ref(&repo), &Exclude::default()).unwrap();
    let renamed = files.iter().find(|f| f.path == "src/new.rs").unwrap();
    // Only the added line is a change; the carried-over lines are context,
    // not a whole-file `@@ -0,0` addition.
    assert!(
        !renamed.patch.contains("@@ -0,0"),
        "patch: {}",
        renamed.patch
    );
    assert!(renamed.patch.contains("+fn z() {}"));
    assert!(renamed.patch.contains("\n fn d() {}"));
    assert!(renamed.base.starts_with("fn a() {}"));
}

#[test]
fn unborn_repo_reviews_untracked_files() {
    let (_dir, repo) = fixture_repo();
    write(&repo, "src/lib.rs", "pub fn f() -> i32 { 1 }\n");

    let files = git::changed_files(std::slice::from_ref(&repo), &Exclude::default()).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "src/lib.rs");
    assert!(files[0].patch.contains("+pub fn f() -> i32 { 1 }"));
}

#[test]
fn diff_ignores_color_and_external_diff_config() {
    let (_dir, repo) = fixture_repo();
    write(&repo, "src/lib.rs", "pub fn f() -> i32 { 1 }\n");
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "seed"]);
    run_git(&repo, &["config", "color.ui", "always"]);
    run_git(&repo, &["config", "diff.external", "false"]);

    write(&repo, "src/lib.rs", "pub fn f() -> i32 { 2 }\n");
    let files = git::changed_files(std::slice::from_ref(&repo), &Exclude::default()).unwrap();
    let patch = &files[0].patch;
    assert!(!patch.contains('\x1b'), "patch has ANSI escapes: {patch:?}");
    assert!(patch.contains("\n@@ -1 +1 @@"), "patch: {patch:?}");
}

#[test]
fn base_content_keeps_leading_whitespace() {
    let (_dir, repo) = fixture_repo();
    let original = "\n\n    // indented header\npub fn f() -> i32 { 1 }\n";
    write(&repo, "src/lib.rs", original);
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "seed"]);

    write(
        &repo,
        "src/lib.rs",
        "\n\n    // indented header\npub fn f() -> i32 { 2 }\n",
    );
    let files = git::changed_files(std::slice::from_ref(&repo), &Exclude::default()).unwrap();
    assert_eq!(files[0].base, original);
}

#[test]
fn unborn_sha256_repo_reviews_untracked_files() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().to_path_buf();
    run_git(&repo, &["init", "-q", "--object-format=sha256"]);
    write(&repo, "src/lib.rs", "pub fn f() -> i32 { 1 }\n");

    let files = git::changed_files(std::slice::from_ref(&repo), &Exclude::default()).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "src/lib.rs");
}

#[test]
fn unresolvable_detached_head_is_an_error_not_unborn() {
    let (_dir, repo) = fixture_repo();
    write(&repo, "src/lib.rs", "pub fn f() -> i32 { 1 }\n");
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "seed"]);
    // Point a detached HEAD at a commit id that does not exist.
    fs::write(
        repo.join(".git/HEAD"),
        "0123456789abcdef0123456789abcdef01234567\n",
    )
    .unwrap();

    assert!(git::changed_files(std::slice::from_ref(&repo), &Exclude::default()).is_err());
}

/// Paths of `files`, sorted.
fn sorted_paths(files: &[momus_review::domain::report::ChangedFile]) -> Vec<&str> {
    let mut paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
    paths.sort();
    paths
}

#[test]
fn base_reviews_a_clean_feature_branch() {
    let (_dir, repo) = fixture_repo();
    let v1 = "pub fn f() -> i32 { 1 }\n";
    write(&repo, "src/lib.rs", v1);
    write(&repo, "src/keep.rs", "pub fn k() {}\n");
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "seed"]);
    run_git(&repo, &["branch", "base"]);

    run_git(&repo, &["checkout", "-q", "-b", "feature"]);
    write(
        &repo,
        "src/lib.rs",
        "pub fn f() -> i32 { 1 }\npub fn g() -> i32 { 2 }\n",
    );
    write(&repo, "src/new.rs", "pub fn n() {}\n");
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "feature work"]);

    // A clean checkout (CI) has nothing to review against HEAD...
    let head = git::changed_files(std::slice::from_ref(&repo), &Exclude::default()).unwrap();
    assert!(head.is_empty(), "unexpected: {:?}", sorted_paths(&head));

    // ...but against the base it is the branch's diff.
    let files =
        git::changed_files_since(std::slice::from_ref(&repo), &Exclude::default(), "base").unwrap();
    assert_eq!(sorted_paths(&files), vec!["src/lib.rs", "src/new.rs"]);

    let lib = files.iter().find(|f| f.path == "src/lib.rs").unwrap();
    assert!(
        lib.patch.contains("+pub fn g() -> i32 { 2 }"),
        "patch: {}",
        lib.patch
    );
    assert_eq!(lib.base, v1, "base content comes from the merge base");

    let new = files.iter().find(|f| f.path == "src/new.rs").unwrap();
    assert!(new.patch.contains("@@ -0,0 +1 @@"), "patch: {}", new.patch);
    assert!(new.base.is_empty());

    // Uncommitted edits to tracked files are reviewed too; untracked files
    // are not part of the branch's diff and are ignored.
    write(&repo, "src/keep.rs", "pub fn k() { todo!() }\n");
    write(&repo, "src/stray.rs", "pub fn s() {}\n");
    let files =
        git::changed_files_since(std::slice::from_ref(&repo), &Exclude::default(), "base").unwrap();
    assert_eq!(
        sorted_paths(&files),
        vec!["src/keep.rs", "src/lib.rs", "src/new.rs"]
    );

    // Without --base the untracked file is still reviewed.
    let head = git::changed_files(std::slice::from_ref(&repo), &Exclude::default()).unwrap();
    assert_eq!(sorted_paths(&head), vec!["src/keep.rs", "src/stray.rs"]);
}

#[test]
fn base_head_parent_anchors_on_the_first_added_line() {
    use momus_review::domain::patch::{first_added_line, parse_hunks};

    let (_dir, repo) = fixture_repo();
    let v1: String = (1..=10).map(|i| format!("pub fn f{i}() {{}}\n")).collect();
    write(&repo, "src/lib.rs", &v1);
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "one"]);

    // Second commit: replace line 6 and append an eleventh line.
    let v2 = v1.replace("pub fn f6() {}\n", "pub fn f6() { 6; }\n") + "pub fn f11() {}\n";
    write(&repo, "src/lib.rs", &v2);
    write(&repo, "src/added.rs", "pub fn a() {}\npub fn b() {}\n");
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "two"]);

    let files =
        git::changed_files_since(std::slice::from_ref(&repo), &Exclude::default(), "HEAD~1")
            .unwrap();
    assert_eq!(sorted_paths(&files), vec!["src/added.rs", "src/lib.rs"]);

    let lib = files.iter().find(|f| f.path == "src/lib.rs").unwrap();
    assert_eq!(lib.base, v1);
    let hunks = parse_hunks(&lib.patch);
    // Three lines of context put the hunk start at line 3; the edit is line 6.
    assert_eq!(hunks[0].start_line, 3, "patch: {}", lib.patch);
    assert_eq!(first_added_line(&hunks[0]), 6);
    // The two edits are close enough to share one hunk.
    assert_eq!(hunks.len(), 1, "patch: {}", lib.patch);

    let added = files.iter().find(|f| f.path == "src/added.rs").unwrap();
    assert_eq!(first_added_line(&parse_hunks(&added.patch)[0]), 1);
}

#[test]
fn base_diffs_from_the_merge_base_not_the_base_tip() {
    let (_dir, repo) = fixture_repo();
    write(&repo, "src/a.rs", "pub fn a() {}\n");
    write(&repo, "src/b.rs", "pub fn b() {}\n");
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "seed"]);
    run_git(&repo, &["branch", "base"]);

    run_git(&repo, &["checkout", "-q", "-b", "feature"]);
    write(&repo, "src/a.rs", "pub fn a() { 1; }\n");
    run_git(&repo, &["commit", "-q", "-am", "feature changes a"]);

    // The base branch moves on after the fork.
    run_git(&repo, &["checkout", "-q", "base"]);
    write(&repo, "src/b.rs", "pub fn b() { 2; }\n");
    run_git(&repo, &["commit", "-q", "-am", "base changes b"]);
    run_git(&repo, &["checkout", "-q", "feature"]);

    // Only the branch's own change: b.rs is not shown as a revert.
    let files =
        git::changed_files_since(std::slice::from_ref(&repo), &Exclude::default(), "base").unwrap();
    assert_eq!(sorted_paths(&files), vec!["src/a.rs"]);
}

#[test]
fn unknown_or_option_like_base_is_a_clear_error() {
    let (_dir, repo) = fixture_repo();
    write(&repo, "src/lib.rs", "pub fn f() {}\n");
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-q", "-m", "seed"]);
    let scopes = std::slice::from_ref(&repo);

    let err = git::changed_files_since(scopes, &Exclude::default(), "no-such-branch").unwrap_err();
    assert!(err.to_string().contains("not found"), "{err:#}");
    assert!(err.to_string().contains("fetch-depth: 0"), "{err:#}");

    let err = git::changed_files_since(scopes, &Exclude::default(), "--output=/tmp/x").unwrap_err();
    assert!(err.to_string().contains("must name a revision"), "{err:#}");
}

#[test]
fn repo_path_with_trailing_space_is_preserved() {
    let dir = tempfile::tempdir().unwrap();
    let repo = dir.path().join("repo ");
    fs::create_dir(&repo).unwrap();
    run_git(&repo, &["init", "-q"]);
    write(&repo, "src/lib.rs", "pub fn f() -> i32 { 1 }\n");

    let files = git::repository_files(std::slice::from_ref(&repo), &Exclude::default()).unwrap();
    assert_eq!(files.len(), 1);
}

#[test]
fn auxiliary_evidence_includes_docs_manifests_renames_and_removals() {
    let (_dir, repo) = fixture_repo();
    write(&repo, "src/old.rs", "pub fn old() {}\n");
    write(&repo, "src/deleted.rs", "pub fn deleted() {}\n");
    write(&repo, "Cargo.toml", "[dependencies]\na=\"1\"\n");
    write(&repo, "README.md", "old()\n");
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-qm", "base"]);
    run_git(&repo, &["mv", "src/old.rs", "src/new.rs"]);
    fs::remove_file(repo.join("src/deleted.rs")).unwrap();
    write(&repo, "README.md", "new()\n");
    write(&repo, "Cargo.toml", "[dependencies]\na=\"2\"\n");
    let evidence = git::repository_evidence(
        std::slice::from_ref(&repo),
        &Exclude::default(),
        None,
        100,
        1000,
        10000,
    )
    .unwrap();
    assert_eq!(evidence.changes.len(), 4);
    let rename = evidence
        .changes
        .iter()
        .find(|c| c.path == "src/new.rs")
        .unwrap();
    assert_eq!(rename.old_path.as_deref(), Some("src/old.rs"));
    assert!(rename.base.contains("pub fn old"));
    let removed = evidence
        .changes
        .iter()
        .find(|c| c.path == "src/deleted.rs")
        .unwrap();
    assert!(removed.content.is_none());
    assert!(removed.base.contains("deleted"));
    assert!(evidence.unknowns.is_empty(), "{:?}", evidence.unknowns);
}

#[test]
fn committed_context_is_bounded_and_rejects_ambiguous_identity() {
    let (_dir, repo) = fixture_repo();
    for index in 0..1600 {
        write(
            &repo,
            &format!("src/file{index}.rs"),
            &format!("pub fn f{index}() {{}}\n"),
        );
    }
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-qm", "many blobs"]);
    let head = git::head_sha(&repo).unwrap();
    let files =
        git::repository_files_at(std::slice::from_ref(&repo), &Exclude::default(), &head).unwrap();
    assert_eq!(files.len(), 1600);
    assert!(
        files
            .iter()
            .any(|file| file.path == "src/file1599.rs" && file.content == "pub fn f1599() {}\n")
    );
    for invalid in ["HEAD", "--help", &head[..7]] {
        assert!(
            git::repository_files_at(std::slice::from_ref(&repo), &Exclude::default(), invalid)
                .is_err()
        );
    }
    write(&repo, "src/huge.rs", &"x".repeat(10_000_001));
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-qm", "oversized context"]);
    let head = git::head_sha(&repo).unwrap();
    let error = git::repository_files_at(std::slice::from_ref(&repo), &Exclude::default(), &head)
        .unwrap_err();
    assert!(error.to_string().contains("inventory limits"));
}

#[test]
fn unreadable_source_baseline_is_never_reinterpreted_as_an_addition() {
    let (_dir, repo) = fixture_repo();
    fs::write(repo.join("source.rs"), [0xff, 0x00, 0x80]).unwrap();
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-qm", "binary source baseline"]);
    write(&repo, "source.rs", "pub fn readable_now() {}\n");
    let error = git::changed_files(std::slice::from_ref(&repo), &Exclude::default()).unwrap_err();
    assert!(format!("{error:#}").contains("base source evidence"));
}

#[test]
fn evidence_total_budget_counts_both_retained_current_copies() {
    let (_dir, repo) = fixture_repo();
    write(&repo, "README.md", "old");
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-qm", "base"]);
    write(&repo, "README.md", "new!");
    for (budget, expected_files) in [(7, 0), (11, 1)] {
        let evidence = git::repository_evidence(
            std::slice::from_ref(&repo),
            &Exclude::default(),
            None,
            100,
            1000,
            budget,
        )
        .unwrap();
        assert_eq!(evidence.files.len(), expected_files);
        assert_eq!(evidence.changes.len(), expected_files);
        assert_eq!(evidence.unknowns.is_empty(), expected_files == 1);
    }
}

#[test]
fn unreadable_removed_base_evidence_does_not_abort_other_changes() {
    let (_dir, repo) = fixture_repo();
    fs::write(repo.join("bun.lockb"), [0xff, 0xfe, 0x80]).unwrap();
    write(&repo, "CHANGELOG.md", "binary\0base");
    write(&repo, "README.md", "old\n");
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-qm", "binary baseline"]);
    fs::remove_file(repo.join("bun.lockb")).unwrap();
    fs::remove_file(repo.join("CHANGELOG.md")).unwrap();
    write(&repo, "README.md", "current\n");
    write(&repo, "extra.rs", "pub fn extra() {}\n");
    let evidence = git::repository_evidence(
        std::slice::from_ref(&repo),
        &Exclude::default(),
        None,
        100,
        1000,
        10000,
    )
    .unwrap();
    assert!(
        evidence
            .unknowns
            .iter()
            .any(|s| s.contains("bun.lockb: unreadable base"))
    );
    assert!(
        evidence
            .unknowns
            .iter()
            .any(|s| s.contains("CHANGELOG.md: binary base"))
    );
    assert_eq!(evidence.changes.len(), 2);
    assert!(
        evidence
            .changes
            .iter()
            .any(|c| c.path == "README.md" && c.base == "old\n")
    );
    assert!(
        evidence
            .changes
            .iter()
            .any(|c| c.path == "extra.rs" && c.base.is_empty())
    );
}

#[test]
fn auxiliary_limits_exclusions_binary_and_unsafe_paths_remain_explicit() {
    use std::os::unix::fs::symlink;
    let (_dir, repo) = fixture_repo();
    write(&repo, "base.rs", "fn base() {}\n");
    write(&repo, ".gitignore", "ignored.rs\n");
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-qm", "base"]);
    write(&repo, "ignored.rs", "secret ignored\n");
    write(&repo, "excluded.rs", "secret excluded\n");
    write(&repo, "huge.rs", &"x".repeat(200));
    write(&repo, "binary.rs", "x\0y");
    symlink("/etc/passwd", repo.join("escape.rs")).unwrap();
    let exclude = Exclude::new(&["excluded.rs".into()]).unwrap();
    let evidence =
        git::repository_evidence(std::slice::from_ref(&repo), &exclude, None, 100, 100, 1000)
            .unwrap();
    assert!(
        evidence
            .files
            .iter()
            .all(|f| !f.path.contains("ignored") && !f.path.contains("excluded"))
    );
    for path in ["huge.rs", "binary.rs", "escape.rs"] {
        assert!(
            evidence.unknowns.iter().any(|s| s.contains(path)),
            "{path}: {:?}",
            evidence.unknowns
        );
    }
    let evidence =
        git::repository_evidence(std::slice::from_ref(&repo), &exclude, None, 1, 1000, 1000)
            .unwrap();
    assert!(evidence.files.len() <= 1);
    assert!(evidence.unknowns.iter().any(|s| s.contains("count limit")));
    let evidence =
        git::repository_evidence(std::slice::from_ref(&repo), &exclude, None, 100, 1000, 1)
            .unwrap();
    assert!(evidence.files.is_empty());
    assert!(
        evidence
            .unknowns
            .iter()
            .any(|s| s.contains("total evidence"))
    );
}

#[test]
fn committed_review_reads_pinned_objects_and_ignores_mutable_context() {
    let (_dir, repo) = fixture_repo();
    write(&repo, "src/lib.rs", "pub fn answer() -> i32 { 1 }\n");
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-qm", "base"]);
    run_git(&repo, &["branch", "base"]);
    write(&repo, "src/lib.rs", "pub fn answer() -> i32 { 2 }\n");
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-qm", "committed"]);
    let head = git::head_sha(&repo).unwrap();
    write(&repo, "src/lib.rs", "pub fn answer() -> i32 { 999 }\n");
    write(&repo, "tests/untracked.rs", "fn fake_context() {}\n");
    let changes = git::changed_files_at(
        std::slice::from_ref(&repo),
        &Exclude::default(),
        "base",
        &head,
    )
    .unwrap();
    assert_eq!(changes.len(), 1);
    assert!(changes[0].patch.contains("{ 2 }"));
    assert!(!changes[0].patch.contains("999"));
    let sources =
        git::repository_files_at(std::slice::from_ref(&repo), &Exclude::default(), &head).unwrap();
    assert_eq!(sources.len(), 1);
    assert!(sources[0].content.contains("{ 2 }"));
    assert!(!git::tracked_checkout_clean(&repo).unwrap());
}

#[test]
fn bounded_evidence_rejects_multiple_checkouts_even_at_equal_heads_and_ignores_untracked_ci_inputs()
{
    let (_dir, repo) = fixture_repo();
    write(&repo, "README.md", "committed\n");
    run_git(&repo, &["add", "-A"]);
    run_git(&repo, &["commit", "-qm", "base"]);
    let second = tempfile::tempdir().unwrap();
    run_git(
        second.path(),
        &["clone", "-q", repo.to_str().unwrap(), "copy"],
    );
    let copy = second.path().join("copy");
    assert_eq!(git::head_sha(&repo).unwrap(), git::head_sha(&copy).unwrap());
    let err = git::repository_evidence(
        &[repo.clone(), copy],
        &Exclude::default(),
        None,
        100,
        1000,
        10000,
    )
    .unwrap_err();
    assert!(err.to_string().contains("one checkout"));
    write(&repo, "fake-CHANGELOG.md", "untracked evidence\n");
    let evidence = git::repository_evidence(
        std::slice::from_ref(&repo),
        &Exclude::default(),
        Some("HEAD"),
        100,
        1000,
        10000,
    )
    .unwrap();
    assert!(evidence.files.iter().all(|f| f.path != "fake-CHANGELOG.md"));
    run_git(&repo, &["mv", "README.md", "README.unrecognized"]);
    let evidence = git::repository_evidence(
        std::slice::from_ref(&repo),
        &Exclude::default(),
        None,
        100,
        1000,
        10000,
    )
    .unwrap();
    assert!(
        evidence
            .unknowns
            .iter()
            .any(|s| s.contains("unsupported destination"))
    );
}
