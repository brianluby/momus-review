//! Git adapter: discovers changed source files under a scope (with unified
//! diffs) and complete source files for codebase scans. Every worktree read
//! is guarded against symlinks, special files, and ancestor-swap races.
//! Mirrors `adapters/git.ts` + `adapters/repository-files.ts`.

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

use crate::adapters::exclude::Exclude;
use crate::domain::patch::patch_for_new_file;
use crate::domain::policy::source_file;
use crate::domain::report::{ChangedFile, SourceFile};

/// Runs `git -C <cwd> <args>` and returns trimmed stdout. No shell.
fn git(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .arg("-C")
        .arg(cwd)
        .args(args)
        .output()
        .with_context(|| format!("git {} failed to run", args.join(" ")))?;
    if !output.status.success() {
        bail!(
            "git {} exited {}: {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Resolves the repository under `scope` to its current `HEAD` commit sha
/// (40 hex chars). Used to key per-sha review history; fails loudly when
/// `scope` is not inside a repository.
pub fn head_sha(scope: &Path) -> Result<String> {
    git(scope, &["rev-parse", "HEAD"])
}

/// Splits NUL-separated git path output. `-z` makes git emit NULs so paths
/// containing spaces or newlines round-trip intact.
fn nul_lines(output: &str) -> Vec<&str> {
    output.split('\0').filter(|l| !l.is_empty()).collect()
}

/// Reads `path` relative to `repo_root` only when it is a regular file inside
/// the repo. Symlinks (`O_NOFOLLOW`), non-regular files, and repo escapes
/// resolve to `None`; unexpected I/O failures propagate so scans fail loudly.
fn read_repo_file(repo_root: &Path, path: &str) -> Result<Option<String>> {
    let absolute = repo_root.join(path);
    if !absolute.starts_with(repo_root) {
        return Ok(None);
    }

    let mut file = match open_guarded(&absolute)? {
        Some(f) => f,
        None => return Ok(None),
    };

    let stat = file
        .metadata()
        .with_context(|| format!("fstat {}", absolute.display()))?;
    if !stat.is_file() {
        return Ok(None);
    }

    // A symlinked ancestor path is fine only if it resolves inside the repo.
    let real = match absolute.canonicalize() {
        Ok(real) => real,
        Err(_) => return Ok(None), // vanished during read: benign race
    };
    if real != absolute && !real.starts_with(repo_root) {
        return Ok(None);
    }

    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .with_context(|| format!("read {}", absolute.display()))?;

    // Ancestor swap between open and read is a live-mutation race; fail loudly
    // rather than misattribute content to the reported path.
    assert_same_file(&absolute, stat.dev(), stat.ino(), stat.size())?;

    // Decode only after confirming the bytes came from the path we opened.
    // Non-UTF-8 source fails loudly rather than being silently garbled or
    // dropped from review coverage.
    let content = String::from_utf8(bytes)
        .map_err(|e| anyhow!("{} is not valid UTF-8: {e}", absolute.display()))?;
    Ok(Some(content))
}

fn open_guarded(absolute: &Path) -> Result<Option<File>> {
    let flags = libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK;
    match OpenOptions::new().read(true).custom_flags(flags).open(absolute) {
        Ok(f) => Ok(Some(f)),
        Err(e) if is_benign_open_error(&e) => Ok(None),
        // Unexpected I/O (e.g. EACCES) must not be swallowed.
        Err(e) => Err(anyhow!("open {} failed: {e}", absolute.display())),
    }
}

fn assert_same_file(absolute: &Path, dev: u64, ino: u64, size: u64) -> Result<()> {
    let current = match std::fs::metadata(absolute) {
        Ok(m) if m.is_file() => Some((m.dev(), m.ino(), m.size())),
        _ => None,
    };
    if current != Some((dev, ino, size)) {
        bail!("File changed during read: {}", absolute.display());
    }
    Ok(())
}

/// Symlink race (ELOOP), vanished path (ENOENT), or a non-openable special
/// file (ENXIO/ENODEV): expected races against a live worktree, safe to skip.
fn is_benign_open_error(e: &std::io::Error) -> bool {
    matches!(
        e.raw_os_error(),
        Some(libc::ELOOP | libc::ENOENT | libc::ENXIO | libc::ENODEV)
    )
}

/// Discovers changed source files across `scopes`: tracked diffs plus
/// untracked contents rendered as all-additions patches. Paths matching
/// `exclude` are skipped before any read; overlapping scopes are deduped.
pub fn changed_files(scopes: &[PathBuf], exclude: &Exclude) -> Result<Vec<ChangedFile>> {
    let mut files = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for scope in scopes {
        for file in changed_files_in_scope(scope, exclude)? {
            if seen.insert(file.path.clone()) {
                files.push(file);
            }
        }
    }
    Ok(files)
}

fn changed_files_in_scope(scope: &Path, exclude: &Exclude) -> Result<Vec<ChangedFile>> {
    let real_scope = scope
        .canonicalize()
        .with_context(|| format!("resolve scope {}", scope.display()))?;
    let repo_root = PathBuf::from(git(&real_scope, &["rev-parse", "--show-toplevel"])?)
        .canonicalize()
        .with_context(|| "resolve repo root")?;
    let relative_scope = relative_scope(&repo_root, &real_scope);

    let tracked_output = git(
        &repo_root,
        &[
            "diff",
            "HEAD",
            "--name-only",
            "-z",
            "--diff-filter=ACMRTUXB",
            "--",
            relative_scope,
        ],
    )?;
    let untracked_output = git(
        &repo_root,
        &["ls-files", "--others", "--exclude-standard", "-z", "--", relative_scope],
    )?;
    let tracked = nul_lines(&tracked_output);
    let untracked = nul_lines(&untracked_output);

    let untracked_set: std::collections::HashSet<&str> = untracked.iter().copied().collect();
    let mut seen = std::collections::HashSet::new();
    let mut files = Vec::new();

    for path in tracked.iter().chain(untracked.iter()).copied() {
        if !source_file().is_match(path) || exclude.is_match(path) || !seen.insert(path) {
            continue;
        }
        if !untracked_set.contains(path) {
            let patch = git(
                &repo_root,
                &["diff", "HEAD", "--unified=3", "--", path],
            )?;
            let base = git(&repo_root, &["show", &format!("HEAD:{path}")]).unwrap_or_default();
            files.push(ChangedFile { path: path.to_string(), patch, base });
        } else if let Some(content) = read_repo_file(&repo_root, path)? {
            files.push(ChangedFile {
                path: path.to_string(),
                patch: patch_for_new_file(&content),
                base: String::new(),
            });
        }
    }
    Ok(files)
}

/// Discovers every non-ignored source file across `scopes` (tracked +
/// untracked). Paths matching `exclude` are skipped before any read;
/// overlapping scopes are deduped.
pub fn repository_files(scopes: &[PathBuf], exclude: &Exclude) -> Result<Vec<SourceFile>> {
    let mut files = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for scope in scopes {
        for file in repository_files_in_scope(scope, exclude)? {
            if seen.insert(file.path.clone()) {
                files.push(file);
            }
        }
    }
    Ok(files)
}

fn repository_files_in_scope(scope: &Path, exclude: &Exclude) -> Result<Vec<SourceFile>> {
    let real_scope = scope
        .canonicalize()
        .with_context(|| format!("resolve scope {}", scope.display()))?;
    let repo_root = PathBuf::from(git(&real_scope, &["rev-parse", "--show-toplevel"])?)
        .canonicalize()
        .with_context(|| "resolve repo root")?;
    let relative_scope = relative_scope(&repo_root, &real_scope);

    let paths_output = git(
        &repo_root,
        &[
            "ls-files",
            "--cached",
            "--others",
            "--exclude-standard",
            "-z",
            "--",
            relative_scope,
        ],
    )?;
    let paths = nul_lines(&paths_output);

    let mut files = Vec::new();
    for path in paths {
        if !source_file().is_match(path) || exclude.is_match(path) {
            continue;
        }
        if let Some(content) = read_repo_file(&repo_root, path)? {
            files.push(SourceFile { path: path.to_string(), content });
        }
    }
    Ok(files)
}

fn relative_scope<'a>(repo_root: &Path, real_scope: &'a Path) -> &'a str {
    real_scope
        .strip_prefix(repo_root)
        .ok()
        .and_then(|p| p.to_str())
        .filter(|s| !s.is_empty())
        .unwrap_or(".")
}