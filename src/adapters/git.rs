//! Git-backed discovery and source evidence for directory scopes in a checkout.
//!
//! [`changed_files`], [`changed_files_since`] and [`repository_files`] observe
//! the working tree; [`changed_files_at`] and [`repository_files_at`] select
//! commit objects with a full pinned identity. Diffs come from Git with external
//! diff drivers, color and text conversion disabled. Direct worktree reads
//! refuse final-component symlinks and non-regular files, check that resolved
//! paths remain in the repository, and compare device/inode/size after reading.
//! These checks do not make a live checkout an atomic snapshot or authenticate
//! Git objects. Callers establish repository trust and review identity.
//!
//! Ordinary source discovery errors on unexpected I/O or invalid UTF-8. The
//! bounded auxiliary inventory instead retains individual unavailable evidence
//! as [`RepositoryEvidence::unknowns`]; callers must inspect that list before
//! claiming a complete comparison.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

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
    if rev.is_empty() || rev.starts_with('-') || head.is_empty() || head.starts_with('-') {
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
            "'{rev}' and '{head}' share no history (a shallow clone lacks it: actions/checkout with fetch-depth: 0)"
        )
    })?;
    Ok(DiffBase::Commit(base))
}

/// Return Git's current `HEAD` identity for the directory `scope`.
///
/// The spelling follows the repository's object format, including 40-character
/// SHA-1 and 64-character SHA-256 identities. This reads `HEAD` at call time;
/// it does not pin subsequent operations or verify checkout cleanliness.
///
/// # Errors
///
/// Errors if Git cannot run, `scope` is not a usable repository directory,
/// `HEAD` cannot resolve (including an unborn repository), or stdout is not UTF-8.
pub fn head_sha(scope: &Path) -> Result<String> {
    git_value(scope, &["rev-parse", "HEAD"])
}

/// Resolve the merge base of the supplied revision and the checkout's `HEAD`.
///
/// This is the reviewed fork point, which can differ from the current tip of
/// `base`. Resolution observes current Git refs and does not verify their
/// provenance or fetch missing history.
///
/// # Errors
///
/// Rejects an empty or option-looking revision, a revision that cannot resolve
/// to a commit, missing common history (including insufficient shallow history),
/// Git failures and non-UTF-8 output.
pub fn review_base_sha(scope: &Path, base: &str) -> Result<String> {
    Ok(merge_base(scope, base)?.rev().to_string())
}

/// Return whether Git's porcelain status has no tracked or untracked entries.
///
/// Runs `status --porcelain --untracked-files=all`; Git-ignored files are not
/// included. This status observation is not a lock or a committed-source
/// attestation. Callers bind and recheck `HEAD` separately around a review.
///
/// # Errors
///
/// Propagates Git execution/status failures and non-UTF-8 output; an inability
/// to establish status is not reported as `true`.
pub fn tracked_checkout_clean(scope: &Path) -> Result<bool> {
    Ok(git(scope, &["status", "--porcelain", "--untracked-files=all"])?.is_empty())
}

/// Locate Git's worktree root for `scope` and canonicalize that filesystem path.
///
/// This preserves path whitespace and resolves filesystem aliases. It does not
/// require a clean checkout or establish ownership of the repository.
///
/// # Errors
///
/// Propagates Git failures, non-UTF-8 root output and filesystem canonicalization
/// errors. A missing path or directory outside a worktree cannot supply a root.
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

/// Discover working-tree source changes against `HEAD`, plus untracked sources.
///
/// A genuinely unborn branch uses Git's object-format-aware empty tree. Tracked
/// changes carry unified diffs and unmodified base bytes; untracked regular
/// files become all-additions patches with an empty base. Deleted paths are not
/// source-review subjects. Only supported source extensions are considered,
/// and explicit exclusions are applied before source content is collected.
///
/// Each existing directory scope is canonicalized and mapped to its checkout.
/// Results use repository-relative paths and deduplicate by that path string,
/// including overlapping scopes. This API does not reject multiple checkouts;
/// callers needing a single identity must establish that separately.
///
/// # Errors
///
/// Propagates scope/repository resolution, Git, invalid UTF-8, unreadable base
/// source, unexpected direct-read I/O and detected file-mutation failures.
/// Missing or unsafe untracked files can be skipped by guarded reads. A broken
/// detached `HEAD` is an error, not an empty-tree fallback.
pub fn changed_files(scopes: &[PathBuf], exclude: &Exclude) -> Result<Vec<ChangedFile>> {
    collect_changed_files(scopes, exclude, None, None)
}

/// Discover tracked working-tree changes from the merge base of `base` and `HEAD`.
///
/// This includes branch commits and uncommitted tracked edits. Untracked files
/// are always omitted, even if Git would otherwise discover them. Source
/// filtering, path deduplication and deletion handling follow [`changed_files`].
/// The baseline is the fork point, not necessarily the current `base` tip; this
/// remains a working-tree read rather than committed-only review.
///
/// # Errors
///
/// In addition to [`changed_files`] errors, rejects invalid/option-looking base
/// revisions or missing common history. This function never fetches a base.
pub fn changed_files_since(
    scopes: &[PathBuf],
    exclude: &Exclude,
    base: &str,
) -> Result<Vec<ChangedFile>> {
    collect_changed_files(scopes, exclude, Some(base), None)
}

/// Discover source diffs between the merge base of `base`/`head` and pinned `head`.
///
/// Each provided scope must share one canonical checkout. `head` must be a
/// full lowercase 40- or 64-character Git identity; symbolic names such as
/// `HEAD` and short IDs are rejected. Nonempty scopes also require it to resolve
/// to a commit. Empty scopes return no files without resolving the object.
/// Both diff and baseline evidence come from Git objects, so worktree edits and untracked
/// files do not enter the result. The checkout itself need not be clean.
/// Source filtering, path deduplication and omitted deletions follow
/// [`changed_files`]. For bounded immutable full-file context, also use
/// [`repository_files_at`].
///
/// # Errors
///
/// Propagates invalid pinned identity, incompatible checkout scopes, invalid
/// base/common history, Git output and unavailable or non-text base evidence.
/// This function does not attest the objects' origin or validate every changed
/// destination's file mode; committed context validation is separate.
pub fn changed_files_at(
    scopes: &[PathBuf],
    exclude: &Exclude,
    base: &str,
    head: &str,
) -> Result<Vec<ChangedFile>> {
    validate_pinned_scopes(scopes, head)?;
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
    let additions_output = git_diff_at(
        &repo_root,
        base_rev,
        head,
        &[
            "-M",
            "--name-only",
            "-z",
            "--diff-filter=A",
            "--",
            relative_scope,
        ],
    )?;
    let additions: std::collections::HashSet<_> =
        nul_lines(&additions_output).into_iter().collect();

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
                DiffBase::Commit(_) if additions.contains(path) => String::new(),
                DiffBase::Commit(rev) => git(&repo_root, &["show", &format!("{rev}:{base_path}")])
                    .with_context(|| format!("unavailable base source evidence {base_path}"))?,
            };
            ensure_source_text(base_path, &base)?;
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

/// Read current tracked and non-ignored untracked source files in directory scopes.
///
/// Supported source and test paths are included; explicit exclusions are
/// applied before direct reads. Contents come from guarded working-tree reads,
/// not the index or `HEAD`, and results deduplicate repository-relative paths
/// across scopes. The operation does not enforce one checkout or verify that
/// all reads came from a single moment.
///
/// # Errors
///
/// Propagates scope/repository/Git failures, invalid UTF-8 source, unexpected
/// I/O and detected file mutation. Guarded reads skip vanished paths, final
/// symlinks and non-regular files instead of adding a partial `SourceFile`.
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

/// Working-tree inventory for bounded auxiliary documentation/dependency analyses.
///
/// A nonempty `unknowns` list records coverage limits or unavailable evidence,
/// even when other files and changes were collected successfully. Its absence
/// is not an authenticity or clean-checkout attestation.
#[derive(Debug, Default)]
pub struct RepositoryEvidence {
    /// Selected current text, excluding deletions and individually skipped files.
    pub files: Vec<SourceFile>,
    /// Comparisons against the selected base, retaining rename paths and deletions.
    pub changes: Vec<crate::domain::repository::RepositoryChange>,
    /// Concrete reasons evidence was omitted, excluded, unsafe or beyond bounds.
    pub unknowns: Vec<String>,
    /// Observed checkout `HEAD`; empty for an inventory with no supplied scopes.
    pub head: String,
}

/// Classify supported source, documentation and dependency-evidence path names.
///
/// This checks extensions and recognized manifest/lockfile/release-note names.
/// It does not read a file, validate content, apply exclusions or guarantee
/// usable text; for example, a selected binary lockfile can still be unknown.
///
/// ```
/// use momus_review::adapters::git::evidence_path;
///
/// assert!(evidence_path("src/lib.rs"));
/// assert!(evidence_path("docs/README.md"));
/// assert!(evidence_path("vendor/Cargo.toml"));
/// assert!(evidence_path("bun.lockb")); // Eligible name, not proof of readable text.
/// assert!(!evidence_path("assets/logo.png"));
/// ```
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

/// Collect bounded current files and base comparisons for auxiliary analyses.
///
/// All directory scopes must resolve to one canonical checkout and observed
/// `HEAD`. With `base`, comparisons use its merge base with `HEAD`, ignore
/// untracked files and still read current tracked worktree contents. Without
/// `base`, comparisons use `HEAD` (or an unborn empty tree) and include untracked
/// paths; resolving the reported head still requires an existing `HEAD`.
/// Renames preserve their old path and deleted files carry `content: None`.
///
/// Paths are visited in sorted order within each scope and deduplicated across
/// scopes. `max_files` bounds candidate-path counting, `max_file_bytes` bounds
/// each current/base text read, and `max_total_bytes` bounds retained text: base
/// bytes plus current bytes, counting current text twice when retained in both
/// `files` and `changes`. The total budget is checked after those texts are
/// read; it is not a total I/O-byte limit.
///
/// Explicit exclusions, unsupported rename destinations, unsafe/unreadable or
/// binary text, missing base evidence and resource-limit omissions are retained
/// as `unknowns`. Other usable files can still be returned. Inspect those
/// reasons before claiming complete auxiliary evidence; the inventory does not
/// establish a clean checkout or an atomic source snapshot.
///
/// # Errors
///
/// Scope/root/head/base resolution, incompatible checkout identities, malformed
/// Git inventory, invalid size metadata and inventory-command failures abort
/// collection. Individual current/base read failures instead produce unknowns.
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
        let untracked_paths: BTreeSet<String> = if base.is_some() {
            BTreeSet::new()
        } else {
            let paths = git(
                &root,
                &[
                    "ls-files",
                    "--others",
                    "--exclude-standard",
                    "-z",
                    "--",
                    relative,
                ],
            )?;
            nul_lines(&paths).into_iter().map(String::from).collect()
        };
        let paths = git(&root, &["ls-files", "--cached", "-z", "--", relative])?;
        let mut inventory: BTreeSet<String> =
            nul_lines(&paths).into_iter().map(String::from).collect();
        inventory.extend(untracked_paths.iter().cloned());
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
            let untracked = content.is_some() && untracked_paths.contains(&path);
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
                        match git(&root, &["show", &object]) {
                            Ok(text) if !text.contains('\0') => previous = text,
                            Ok(_) => {
                                out.unknowns.push(format!("{old}: binary base evidence"));
                                continue;
                            }
                            Err(e) => {
                                out.unknowns
                                    .push(format!("{old}: unreadable base evidence: {e}"));
                                continue;
                            }
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
                .saturating_add(content.as_ref().map_or(0, |text| {
                    // Changed current text is retained in both the inventory
                    // and comparison; count both copies in the evidence budget.
                    text.len()
                        .saturating_mul(if change.is_some() || untracked { 2 } else { 1 })
                }));
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

/// Read bounded source/test context entirely from a pinned commit's Git blobs.
///
/// Each provided directory scope must belong to one canonical checkout. `head`
/// must be a full lowercase 40- or 64-character identity, not a ref name or
/// abbreviation. Nonempty scopes require a commit object; empty scopes return
/// no files without resolving that object. Current worktree/index contents and
/// untracked files, including ignored worktree artifacts, are not read. Source paths matching
/// `exclude` are omitted before blob reads; results deduplicate
/// repository-relative paths across scopes.
///
/// Only regular blob modes `100644` and `100755` are accepted for selected
/// source paths. The complete inventory is limited to 10,000 selected files,
/// 10,000,000 bytes per blob and 100,000,000 bytes total. Batch reads check blob
/// identity, advertised size, terminators, UTF-8 and absence of NUL bytes. This
/// preserves pinned context consistency, not evidence of who authored or
/// authorized the commit. The checkout need not be clean.
///
/// # Errors
///
/// Returns an error on invalid identity, scopes from different checkouts, Git
/// failures, unsupported file modes, exceeded bounds or malformed/non-text
/// blob evidence. Unlike bounded auxiliary collection, incomplete committed
/// context is not returned as a successful partial inventory.
pub fn repository_files_at(
    scopes: &[PathBuf],
    exclude: &Exclude,
    head: &str,
) -> Result<Vec<SourceFile>> {
    validate_pinned_scopes(scopes, head)?;
    let mut out = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    let mut total_bytes = 0usize;
    for scope in scopes {
        let scope = scope.canonicalize()?;
        let root = repository_root(&scope)?;
        let relative = relative_scope(&root, &scope);
        let tree = git(&root, &["ls-tree", "-r", "-l", "-z", head, "--", relative])?;
        let mut blobs = Vec::new();
        for record in nul_lines(&tree) {
            let (metadata, path) = record
                .split_once('\t')
                .context("invalid Git tree evidence")?;
            if !is_source_path(path) || exclude.is_match(path) || !seen.insert(path.to_string()) {
                continue;
            }
            if !metadata.starts_with("100644 blob ") && !metadata.starts_with("100755 blob ") {
                bail!("{path}: unsupported non-regular committed source context");
            }
            let fields: Vec<_> = metadata.split_whitespace().collect();
            let blob = fields.get(2).context("missing Git blob")?;
            let size = fields
                .get(3)
                .context("missing Git blob size")?
                .parse::<usize>()?;
            total_bytes = total_bytes.saturating_add(size);
            if seen.len() > 10_000 || size > 10_000_000 || total_bytes > 100_000_000 {
                bail!(
                    "committed source context exceeds inventory limits (10,000 files, 10 MB per file, 100 MB total); complete review unavailable"
                );
            }
            blobs.push((path.to_string(), blob.to_string(), size));
        }
        if !blobs.is_empty() {
            out.extend(read_blob_batch(&root, &blobs)?);
        }
    }
    Ok(out)
}

fn validate_pinned_scopes(scopes: &[PathBuf], head: &str) -> Result<()> {
    if !matches!(head.len(), 40 | 64)
        || !head
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        bail!("pinned head must be a full lowercase Git commit identity");
    }
    let mut identity = None;
    for scope in scopes {
        let root = repository_root(scope)?;
        if identity.as_ref().is_some_and(|previous| previous != &root) {
            bail!("committed source scopes must share one canonical checkout");
        }
        if identity.is_none() && git_value(&root, &["cat-file", "-t", head])? != "commit" {
            bail!("pinned head must identify a commit");
        }
        identity = Some(root);
    }
    Ok(())
}

fn read_blob_batch(root: &Path, blobs: &[(String, String, usize)]) -> Result<Vec<SourceFile>> {
    let input: String = blobs
        .iter()
        .map(|(_, blob, _)| format!("{blob}\n"))
        .collect();
    let mut child = Command::new("git")
        .current_dir(root)
        .args(["cat-file", "--batch"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let mut stdin = child.stdin.take().context("missing Git batch input")?;
    // Drain stdout while feeding stdin to avoid pipe-buffer deadlock on large inventories.
    let writer = std::thread::spawn(move || stdin.write_all(input.as_bytes()));
    let output = child.wait_with_output()?;
    writer
        .join()
        .map_err(|_| anyhow!("Git batch input writer failed"))??;
    if !output.status.success() {
        bail!(
            "Git batch source read failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    let mut cursor = 0usize;
    let mut files = Vec::new();
    for (path, blob, size) in blobs {
        let line_end = output.stdout[cursor..]
            .iter()
            .position(|&b| b == b'\n')
            .map(|offset| cursor + offset)
            .context("missing Git batch header")?;
        let header = std::str::from_utf8(&output.stdout[cursor..line_end])?;
        let expected = format!("{blob} blob {size}");
        if header != expected {
            bail!("{path}: Git batch blob identity/size mismatched");
        }
        cursor = line_end + 1;
        let end = cursor
            .checked_add(*size)
            .context("Git batch size overflow")?;
        let bytes = output
            .stdout
            .get(cursor..end)
            .context("truncated Git batch source")?;
        let content = std::str::from_utf8(bytes)
            .with_context(|| format!("{path}: non-UTF-8 committed source"))?
            .to_owned();
        ensure_source_text(path, &content)?;
        if output.stdout.get(end) != Some(&b'\n') {
            bail!("{path}: missing Git batch terminator");
        }
        cursor = end + 1;
        files.push(SourceFile {
            path: path.clone(),
            content,
        });
    }
    if cursor != output.stdout.len() {
        bail!("unexpected trailing Git batch data");
    }
    Ok(files)
}
fn ensure_source_text(path: &str, text: &str) -> Result<()> {
    if text.contains('\0') {
        bail!("{path}: binary source evidence");
    }
    Ok(())
}
