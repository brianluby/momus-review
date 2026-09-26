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
use crate::domain::language::is_source_path;
use crate::domain::report::{ChangedFile, SourceFile};

/// Flags that pin `git diff` to plain unified output regardless of user or
/// repo config (`diff.external`, `color.ui=always`, textconv drivers), which
/// would otherwise produce output `parse_hunks` cannot read.
const PLAIN_DIFF: [&str; 3] = ["--no-ext-diff", "--no-color", "--no-textconv"];

/// Runs `git -C <cwd> <args>` and returns stdout verbatim. No shell. Callers
/// that want a single trimmed value (e.g. `rev-parse`) use `git_value`;
/// file content must not be trimmed or leading lines would be lost.
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
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// `git` for single-value commands (`rev-parse`): strips only git's
/// terminating newline, so a repo path with leading/trailing spaces survives.
fn git_value(cwd: &Path, args: &[&str]) -> Result<String> {
    let out = git(cwd, args)?;
    Ok(out.strip_suffix('\n').unwrap_or(&out).to_string())
}

/// Runs `git diff <base> <PLAIN_DIFF> <args>`.
fn git_diff(cwd: &Path, base: &str, args: &[&str]) -> Result<String> {
    let mut full = vec!["diff", base];
    full.extend(PLAIN_DIFF);
    full.extend(args);
    git(cwd, &full)
}

/// What changes are diffed against.
enum DiffBase {
    Head,
    /// Unborn `HEAD`: the repository's empty-tree id (object-format aware).
    EmptyTree(String),
}

impl DiffBase {
    fn rev(&self) -> &str {
        match self {
            DiffBase::Head => "HEAD",
            DiffBase::EmptyTree(id) => id,
        }
    }
}

/// The diff base: `HEAD`, or the empty tree when the repository has no
/// commits yet (so an unborn repo still reviews its untracked files). Falls
/// back only for a genuinely unborn `HEAD` (a symbolic ref to a branch that
/// does not exist yet); a detached or corrupt `HEAD` that fails to resolve is
/// an error, not an empty repository.
fn diff_base(repo_root: &Path) -> Result<DiffBase> {
    if git(repo_root, &["rev-parse", "--verify", "--quiet", "HEAD"]).is_ok() {
        return Ok(DiffBase::Head);
    }
    let branch = git_value(repo_root, &["symbolic-ref", "--quiet", "HEAD"])
        .context("HEAD does not resolve to a commit and is not a branch ref")?;
    if git(repo_root, &["show-ref", "--verify", "--quiet", &branch]).is_ok() {
        bail!("HEAD points to {branch}, which exists but does not resolve to a commit");
    }
    // `hash-object` yields the empty-tree id in the repo's own object format
    // (SHA-1 or SHA-256); stdin is closed, so it hashes an empty tree.
    let empty_tree = git_value(repo_root, &["hash-object", "-t", "tree", "--stdin"])?;
    Ok(DiffBase::EmptyTree(empty_tree))
}

/// Resolves the repository under `scope` to its current `HEAD` commit sha
/// (40 hex chars). Used to key per-sha review history; fails loudly when
/// `scope` is not inside a repository.
pub fn head_sha(scope: &Path) -> Result<String> {
    git_value(scope, &["rev-parse", "HEAD"])
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
    let repo_root = PathBuf::from(git_value(&real_scope, &["rev-parse", "--show-toplevel"])?)
        .canonicalize()
        .with_context(|| "resolve repo root")?;
    let relative_scope = relative_scope(&repo_root, &real_scope);

    let base_kind = diff_base(&repo_root)?;
    let base_rev = base_kind.rev();

    // Rename destinations don't exist at the base; remember new→old so both
    // the patch and the base lookup can use the pre-change path. `-z` keeps
    // unusual paths unquoted: records are `R<score>\0old\0new\0`.
    let mut renamed: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let rename_output = git_diff(
        &repo_root,
        base_rev,
        &["-M", "--name-status", "-z", "--diff-filter=R", "--", relative_scope],
    )?;
    let mut fields = nul_lines(&rename_output).into_iter();
    while let (Some(status), Some(old), Some(new)) = (fields.next(), fields.next(), fields.next()) {
        if status.starts_with('R') {
            renamed.insert(new.to_string(), old.to_string());
        }
    }

    let tracked_output = git_diff(
        &repo_root,
        base_rev,
        &["--name-only", "-z", "--diff-filter=ACMRTUXB", "--", relative_scope],
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
        if !is_source_path(path) || exclude.is_match(path) || !seen.insert(path) {
            continue;
        }
        if !untracked_set.contains(path) {
            // A rename's diff must name both paths, or git cannot pair them
            // and renders the destination as an all-additions new file.
            let base_path = renamed.get(path).map(String::as_str).unwrap_or(path);
            let mut diff_args = vec!["-M", "--unified=3", "--", path];
            if base_path != path {
                diff_args.push(base_path);
            }
            let patch = git_diff(&repo_root, base_rev, &diff_args)?.trim_end().to_string();
            let base = match base_kind {
                DiffBase::EmptyTree(_) => String::new(),
                DiffBase::Head => {
                    git(&repo_root, &["show", &format!("HEAD:{base_path}")]).unwrap_or_default()
                }
            };
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
    let repo_root = PathBuf::from(git_value(&real_scope, &["rev-parse", "--show-toplevel"])?)
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
        if !is_source_path(path) || exclude.is_match(path) {
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