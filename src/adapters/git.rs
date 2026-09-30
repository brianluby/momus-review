//! Git adapter: discovers changed source files under a scope (with unified
//! diffs) and complete source files for codebase scans. Every worktree read
//! is guarded against symlinks, special files, and ancestor-swap races.

use std::fs::{File, OpenOptions};
use std::io::Read;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail};

use crate::adapters::exclude::Exclude;
use crate::domain::language::is_source_path;
use crate::domain::patch::patch_for_new_file;
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
    String::from_utf8(output.stdout).context("Git evidence is not valid UTF-8")
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
    /// A commit: `HEAD`, or the merge base with `--base`. File content before
    /// the change is read from it.
    Commit(String),
    /// Unborn `HEAD`: the repository's empty-tree id (object-format aware).
    EmptyTree(String),
}

impl DiffBase {
    fn rev(&self) -> &str {
        match self {
            DiffBase::Commit(rev) | DiffBase::EmptyTree(rev) => rev,
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
        return Ok(DiffBase::Commit("HEAD".to_string()));
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

/// The `--base` diff base: the merge base of `rev` and `HEAD`, so the diff is
/// what the branch changed since it forked (a PR's diff), not everything
/// `rev` gained since. A `rev` that looks like an option is rejected before
/// it reaches git.
fn merge_base(repo_root: &Path, rev: &str) -> Result<DiffBase> {
    merge_base_at(repo_root, rev, "HEAD")
}

fn merge_base_at(repo_root: &Path, rev: &str, head: &str) -> Result<DiffBase> {
    if rev.is_empty() || rev.starts_with('-') {
        bail!("--base must name a revision, got '{rev}'");
    }
    let commit = git_value(
        repo_root,
        &["rev-parse", "--verify", "--quiet", &format!("{rev}^{{commit}}")],
    )
    .map_err(|_| {
        anyhow!(
            "base revision '{rev}' not found in {} (in CI, fetch it: actions/checkout with fetch-depth: 0)",
            repo_root.display()
        )
    })?;
    let base = git_value(repo_root, &["merge-base", &commit, head]).map_err(|_| {
        anyhow!(
            "'{rev}' and HEAD share no history (a shallow clone lacks it: actions/checkout with fetch-depth: 0)"
        )
    })?;
    Ok(DiffBase::Commit(base))
}

/// Resolves the repository under `scope` to its current `HEAD` commit sha
/// (40 hex chars). Used to key per-sha review history; fails loudly when
/// `scope` is not inside a repository.
pub fn head_sha(scope: &Path) -> Result<String> {
    git_value(scope, &["rev-parse", "HEAD"])
}

pub fn review_base_sha(scope: &Path, base: &str) -> Result<String> {
    Ok(merge_base(scope, base)?.rev().to_string())
}

pub fn tracked_checkout_clean(scope: &Path) -> Result<bool> {
    Ok(git(scope, &["status", "--porcelain", "--untracked-files=all"])?.is_empty())
}

pub fn repository_root(scope: &Path) -> Result<PathBuf> {
    Ok(PathBuf::from(git_value(scope, &["rev-parse", "--show-toplevel"])?).canonicalize()?)
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
    read_repo_file_limit(repo_root, path, usize::MAX)
}

fn read_repo_file_limit(repo_root: &Path, path: &str, limit: usize) -> Result<Option<String>> {
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
    if stat.len() > limit as u64 {
        bail!("{path}: file exceeds evidence byte limit {limit}");
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
    (&mut file)
        .take((limit as u64).saturating_add(1))
        .read_to_end(&mut bytes)
        .with_context(|| format!("read {}", absolute.display()))?;
    if bytes.len() > limit {
        bail!("{path}: file grew beyond evidence byte limit");
    }

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
    match OpenOptions::new()
        .read(true)
        .custom_flags(flags)
        .open(absolute)
    {
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
    collect_changed_files(scopes, exclude, None, None)
}

/// `changed_files` against the merge base of `base` and `HEAD` instead of
/// `HEAD` (`momus review --base`): the branch's commits plus uncommitted
/// changes to tracked files; untracked files are ignored. On a clean CI
/// checkout of a PR head that is exactly the PR's diff.
pub fn changed_files_since(
    scopes: &[PathBuf],
    exclude: &Exclude,
    base: &str,
) -> Result<Vec<ChangedFile>> {
    collect_changed_files(scopes, exclude, Some(base), None)
}

pub fn changed_files_at(
    scopes: &[PathBuf],
    exclude: &Exclude,
    base: &str,
    head: &str,
) -> Result<Vec<ChangedFile>> {
    collect_changed_files(scopes, exclude, Some(base), Some(head))
}

fn collect_changed_files(
    scopes: &[PathBuf],
    exclude: &Exclude,
    base: Option<&str>,
    head: Option<&str>,
) -> Result<Vec<ChangedFile>> {
    let mut files = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for scope in scopes {
        for file in changed_files_in_scope(scope, exclude, base, head)? {
            if seen.insert(file.path.clone()) {
                files.push(file);
            }
        }
    }
    Ok(files)
}

fn changed_files_in_scope(
    scope: &Path,
    exclude: &Exclude,
    base: Option<&str>,
    head: Option<&str>,
) -> Result<Vec<ChangedFile>> {
    let real_scope = scope
        .canonicalize()
        .with_context(|| format!("resolve scope {}", scope.display()))?;
    let repo_root = PathBuf::from(git_value(&real_scope, &["rev-parse", "--show-toplevel"])?)
        .canonicalize()
        .with_context(|| "resolve repo root")?;
    let relative_scope = relative_scope(&repo_root, &real_scope);

    let base_kind = match base {
        Some(rev) => merge_base_at(&repo_root, rev, head.unwrap_or("HEAD"))?,
        None => diff_base(&repo_root)?,
    };
    let base_rev = base_kind.rev();

    // Rename destinations don't exist at the base; remember new→old so both
    // the patch and the base lookup can use the pre-change path. `-z` keeps
    // unusual paths unquoted: records are `R<score>\0old\0new\0`.
    let mut renamed: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    let rename_output = git_diff_at(
        &repo_root,
        base_rev,
        head,
        &[
            "-M",
            "--name-status",
            "-z",
            "--diff-filter=R",
            "--",
            relative_scope,
        ],
    )?;
    let mut fields = nul_lines(&rename_output).into_iter();
    while let (Some(status), Some(old), Some(new)) = (fields.next(), fields.next(), fields.next()) {
        if status.starts_with('R') {
            renamed.insert(new.to_string(), old.to_string());
        }
    }

    let tracked_output = git_diff_at(
        &repo_root,
        base_rev,
        head,
        &[
            "--name-only",
            "-z",
            "--diff-filter=ACMRTUXB",
            "--",
            relative_scope,
        ],
    )?;
    // With `--base` the review is the branch's diff: untracked files are not
    // part of it (in CI they are build or checkout leftovers), so skip them.
    let untracked_output = match base {
        Some(_) => String::new(),
        None => git(
            &repo_root,
            &[
                "ls-files",
                "--others",
                "--exclude-standard",
                "-z",
                "--",
                relative_scope,
            ],
        )?,
    };
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
            let patch = git_diff_at(&repo_root, base_rev, head, &diff_args)?
                .trim_end()
                .to_string();
            let base = match &base_kind {
                DiffBase::EmptyTree(_) => String::new(),
                DiffBase::Commit(rev) => {
                    git(&repo_root, &["show", &format!("{rev}:{base_path}")]).unwrap_or_default()
                }
            };
            files.push(ChangedFile {
                path: path.to_string(),
                patch,
                base,
            });
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
            files.push(SourceFile {
                path: path.to_string(),
                content,
            });
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

/// Optional analyses share one bounded inventory, including documentation,
/// dependency files and deletions that ordinary source review omits.
#[derive(Debug, Default)]
pub struct RepositoryEvidence {
    pub files: Vec<SourceFile>,
    pub changes: Vec<crate::domain::repository::RepositoryChange>,
    pub unknowns: Vec<String>,
    pub head: String,
}

pub fn evidence_path(path: &str) -> bool {
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    is_source_path(path)
        || name.ends_with(".md")
        || name.ends_with(".mdx")
        || name.ends_with(".markdown")
        || name.ends_with(".rst")
        || name.ends_with(".adoc")
        || name.ends_with(".csproj")
        || (name.starts_with("requirements") && name.ends_with(".txt"))
        || name.contains("changelog")
        || name.contains("release-notes")
        || matches!(
            name.as_str(),
            "cargo.toml"
                | "cargo.lock"
                | "package.json"
                | "package-lock.json"
                | "npm-shrinkwrap.json"
                | "yarn.lock"
                | "pnpm-lock.yaml"
                | "pyproject.toml"
                | "poetry.lock"
                | "uv.lock"
                | "requirements.txt"
                | "go.mod"
                | "go.sum"
                | "gemfile"
                | "gemfile.lock"
                | "composer.json"
                | "composer.lock"
                | "pom.xml"
                | "build.gradle"
                | "build.gradle.kts"
                | "pipfile"
                | "pipfile.lock"
                | "bun.lock"
                | "bun.lockb"
                | "packages.lock.json"
                | "package.swift"
                | "package.resolved"
                | "mix.exs"
                | "mix.lock"
                | "deno.json"
                | "deno.jsonc"
                | "deno.lock"
        )
}

/// Deterministic resource limits apply before file reads. Unknown/skipped
/// evidence is reported, never converted to an empty successful comparison.
pub fn repository_evidence(
    scopes: &[PathBuf],
    exclude: &Exclude,
    base: Option<&str>,
    max_files: usize,
    max_file_bytes: usize,
    max_total_bytes: usize,
) -> Result<RepositoryEvidence> {
    use crate::domain::repository::RepositoryChange;
    use std::collections::{BTreeMap, BTreeSet};
    let mut out = RepositoryEvidence::default();
    let mut seen = BTreeSet::new();
    let mut total = 0usize;
    let mut identity = None;
    for scope in scopes {
        let scope = scope.canonicalize()?;
        let root =
            PathBuf::from(git_value(&scope, &["rev-parse", "--show-toplevel"])?).canonicalize()?;
        let head = head_sha(&root)?;
        if identity.as_ref().is_some_and(|previous| previous != &root) {
            bail!("optional repository analyses require scopes from one checkout");
        }
        identity = Some(root.clone());
        if !out.head.is_empty() && out.head != head {
            bail!("optional repository analyses require scopes from one repository head");
        }
        out.head = head;
        let relative = relative_scope(&root, &scope);
        let rev = match base {
            Some(b) => merge_base(&root, b)?,
            None => diff_base(&root)?,
        };
        let status = git_diff(
            &root,
            rev.rev(),
            &["-M", "--name-status", "-z", "--", relative],
        )?;
        let mut fields = nul_lines(&status).into_iter();
        let mut changes = BTreeMap::new();
        while let Some(kind) = fields.next() {
            let Some(first) = fields.next() else {
                bail!("malformed Git name-status evidence");
            };
            let (path, old_path) = if kind.starts_with('R') || kind.starts_with('C') {
                (
                    fields.next().context("missing rename destination")?,
                    Some(first.to_string()),
                )
            } else {
                (first, None)
            };
            changes.insert(
                path.to_string(),
                (kind.starts_with('D'), old_path, kind.starts_with('A')),
            );
        }
        let paths = if base.is_some() {
            git(&root, &["ls-files", "--cached", "-z", "--", relative])?
        } else {
            git(
                &root,
                &[
                    "ls-files",
                    "--cached",
                    "--others",
                    "--exclude-standard",
                    "-z",
                    "--",
                    relative,
                ],
            )?
        };
        let mut inventory: BTreeSet<String> =
            nul_lines(&paths).into_iter().map(String::from).collect();
        inventory.extend(changes.keys().cloned());
        for path in inventory {
            if !evidence_path(&path)
                && changes
                    .get(&path)
                    .and_then(|c| c.1.as_deref())
                    .is_some_and(evidence_path)
            {
                out.unknowns.push(format!(
                    "{path}: evidence was renamed to an unsupported destination"
                ));
            }
            if !evidence_path(&path) || !seen.insert(path.clone()) {
                continue;
            }
            if exclude.is_match(&path) {
                let note = "Explicit exclusions narrowed repository evidence coverage".to_string();
                if !out.unknowns.contains(&note) {
                    out.unknowns.push(note);
                }
                continue;
            }
            if seen.len() > max_files {
                out.unknowns.push(format!(
                    "evidence file count limit {max_files} reached; remaining paths omitted"
                ));
                break;
            }
            let change = changes.get(&path);
            let removed = change.is_some_and(|c| c.0);
            let content = if removed {
                None
            } else {
                match read_repo_file_limit(&root, &path, max_file_bytes) {
                    Ok(Some(text)) if !text.contains('\0') => Some(text),
                    Ok(_) => {
                        out.unknowns
                            .push(format!("{path}: unreadable, binary or unsafe evidence"));
                        continue;
                    }
                    Err(e) => {
                        out.unknowns.push(format!("{path}: {e}"));
                        continue;
                    }
                }
            };
            let untracked = base.is_none()
                && content.is_some()
                && git(&root, &["ls-files", "--error-unmatch", "--", &path]).is_err();
            let mut previous = String::new();
            if change.is_some() {
                let old = change.and_then(|c| c.1.as_deref()).unwrap_or(&path);
                if exclude.is_match(old) {
                    out.unknowns.push(format!("{path}: previous path excluded"));
                    continue;
                }
                let object = format!("{}:{old}", rev.rev());
                match git_value(&root, &["cat-file", "-s", &object]) {
                    Ok(size) => {
                        let size = size.parse::<usize>()?;
                        if size > max_file_bytes {
                            out.unknowns
                                .push(format!("{old}: base exceeds evidence byte limit"));
                            continue;
                        }
                        previous = git(&root, &["show", &object])?;
                        if previous.contains('\0') {
                            out.unknowns.push(format!("{old}: binary base evidence"));
                            continue;
                        }
                    }
                    Err(_) if change.is_some_and(|c| c.2) => {} // actual addition has no previous blob
                    Err(e) => {
                        out.unknowns
                            .push(format!("{old}: unavailable base evidence: {e}"));
                        continue;
                    }
                }
            }
            let bytes = previous
                .len()
                .saturating_add(content.as_ref().map_or(0, String::len));
            if total.saturating_add(bytes) > max_total_bytes {
                out.unknowns.push(format!(
                    "{path}: total evidence byte limit {max_total_bytes} reached"
                ));
                continue;
            }
            total += bytes;
            if let Some(text) = &content {
                out.files.push(SourceFile {
                    path: path.clone(),
                    content: text.clone(),
                });
            }
            if change.is_some() || untracked {
                out.changes.push(RepositoryChange {
                    path,
                    old_path: change.and_then(|c| c.1.clone()),
                    base: previous,
                    content,
                });
            }
        }
    }
    Ok(out)
}

fn git_diff_at(cwd: &Path, base: &str, head: Option<&str>, args: &[&str]) -> Result<String> {
    if let Some(head) = head {
        let mut full = vec!["diff", base, head];
        full.extend(PLAIN_DIFF);
        full.extend(args);
        git(cwd, &full)
    } else {
        git_diff(cwd, base, args)
    }
}

/// Source/test context read entirely from one pinned immutable Git tree.
pub fn repository_files_at(
    scopes: &[PathBuf],
    exclude: &Exclude,
    head: &str,
) -> Result<Vec<SourceFile>> {
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for scope in scopes {
        let scope = scope.canonicalize()?;
        let root = repository_root(&scope)?;
        let relative = relative_scope(&root, &scope);
        let tree = git(&root, &["ls-tree", "-r", "-z", head, "--", relative])?;
        for record in nul_lines(&tree) {
            let (metadata, path) = record
                .split_once('\t')
                .context("invalid Git tree evidence")?;
            if !metadata.starts_with("100644 blob ") && !metadata.starts_with("100755 blob ") {
                continue;
            }
            if !is_source_path(path) || exclude.is_match(path) || !seen.insert(path.to_string()) {
                continue;
            }
            let blob = metadata
                .split_whitespace()
                .nth(2)
                .context("missing Git blob")?;
            let content = git(&root, &["cat-file", "blob", blob])?;
            ensure_source_text(path, &content)?;
            out.push(SourceFile {
                path: path.into(),
                content,
            });
        }
    }
    Ok(out)
}
fn ensure_source_text(path: &str, text: &str) -> Result<()> {
    if text.contains('\0') {
        bail!("{path}: binary source evidence");
    }
    Ok(())
}
