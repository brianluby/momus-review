//! Conservative, local documentation/interface comparisons (#22).
//!
//! A finding requires a Rust example tied to a particular source file and a
//! directly observed fixed-arity mismatch or removed public declaration.
//! Absence from a bounded repository inventory never proves an API is absent.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::domain::policy::Dimension;
use crate::domain::redact::{Redactions, redact};
use crate::domain::report::{Action, Finding, SourceFile};
use crate::domain::repository::RepositoryChange;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocsEvidence {
    pub path: String,
    pub line: usize,
    /// `base` evidence is pre-change; it must never be presented as current.
    pub revision: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocsCheck {
    pub status: String,
    pub symbol: String,
    pub documentation: DocsEvidence,
    pub source: Option<DocsEvidence>,
    pub reason: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct DocsDriftSummary {
    pub findings: Vec<Finding>,
    pub unknowns: Vec<String>,
    pub checks: Vec<DocsCheck>,
}

#[derive(Debug, Clone)]
struct Interface {
    name: String,
    arity: Option<usize>,
    evidence: DocsEvidence,
}

struct PreparedExample {
    line: usize,
    original: String,
    clean: String,
    shadowed: BTreeSet<String>,
    wildcard_import: bool,
}

struct PreparedDocument<'a> {
    path: &'a str,
    links: BTreeSet<String>,
    examples: Vec<PreparedExample>,
}

static DECLARATION: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^pub\s+(?:(?:async|unsafe|const)\s+)*fn\s+([A-Za-z_][A-Za-z0-9_]*)\s*\(([^()]*)\)")
        .expect("valid declaration pattern")
});
static CALL: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\b((?:[A-Za-z_][A-Za-z0-9_]*::)*)([A-Za-z_][A-Za-z0-9_]*)\s*\(([^()]*)\)")
        .expect("valid call pattern")
});
static SIMPLE_PARAMETER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:mut\s+)?[A-Za-z_][A-Za-z0-9_]*\s*:\s*(?:&\s*(?:'[A-Za-z_][A-Za-z0-9_]*\s+)?(?:mut\s+)?)?[A-Za-z_][A-Za-z0-9_:]*\s*$")
        .expect("valid simple parameter pattern")
});
static LINK: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\]\(([^\s)]+)\)").expect("valid Markdown link pattern"));
static SOURCE_REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"`([^`\s]+\.rs)`").expect("valid inline source reference pattern")
});
static IMPORT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\buse\b[^;]*;").expect("valid Rust example import pattern"));

/// Compare current Markdown examples with visible public Rust functions.
///
/// `files` is a bounded current inventory. Changes override that inventory,
/// including deletions. The caller records exclusions/budget drops as unknowns
/// and feeds findings through the ordinary fingerprint/suppression pipeline.
pub fn assess(changes: &[RepositoryChange], files: &[SourceFile]) -> DocsDriftSummary {
    let mut summary = DocsDriftSummary::default();
    let mut current: BTreeMap<&str, &str> = files
        .iter()
        .map(|file| (file.path.as_str(), file.content.as_str()))
        .collect();
    for change in changes {
        if let Some(old) = change.old_path.as_deref() {
            current.remove(old);
        }
        if let Some(content) = change.content.as_deref() {
            current.insert(&change.path, content);
        } else {
            current.remove(change.path.as_str());
        }
    }
    let has_code_changes = changes.iter().any(|change| !is_document(&change.path));
    let changed_docs: BTreeSet<&str> = changes
        .iter()
        .filter(|change| is_document(&change.path))
        .map(|change| change.path.as_str())
        .collect();
    // Prepare each relevant document once. Unsupported declarations are useful
    // evidence only in a source file to which these documents establish identity.
    let mut relevant_sources = BTreeSet::new();
    let mut documents = Vec::new();
    for (&path, &content) in &current {
        if !is_document(path) || (!has_code_changes && !changed_docs.contains(path)) {
            continue;
        }
        let links = source_links(path, content);
        relevant_sources.extend(links.iter().cloned());
        let examples = examples(path, content, &mut summary.unknowns)
            .into_iter()
            .map(|(line, original)| {
                let clean = lexical_code(&original);
                for captures in CALL.captures_iter(&clean) {
                    add_qualified_sources(&captures[1], &mut relevant_sources);
                }
                let (shadowed, wildcard_import) = example_shadows(&clean);
                PreparedExample {
                    line,
                    original,
                    clean,
                    shadowed,
                    wildcard_import,
                }
            })
            .collect();
        documents.push(PreparedDocument {
            path,
            links,
            examples,
        });
    }
    let mut interfaces: BTreeMap<String, Vec<Interface>> = BTreeMap::new();
    let mut old_interfaces: BTreeMap<String, Vec<Interface>> = BTreeMap::new();
    let mut current_identifiers = BTreeSet::new();
    let mut opaque_exports = false;
    for (&path, &content) in &current {
        if path.ends_with(".rs") {
            let clean = lexical_code(content);
            current_identifiers.extend(identifiers(&clean).map(str::to_owned));
            opaque_exports |= has_opaque_exports(&clean);
            let mut unknowns = Vec::new();
            for interface in extract_interfaces(path, content, &clean, "current", &mut unknowns) {
                interfaces
                    .entry(interface.name.clone())
                    .or_default()
                    .push(interface);
            }
            if relevant_sources.contains(path) {
                summary.unknowns.extend(unknowns);
            }
        }
    }
    for change in changes {
        if change.path.ends_with(".rs") {
            let path = change.old_path.as_deref().unwrap_or(&change.path);
            let clean = lexical_code(&change.base);
            let mut unknowns = Vec::new();
            for interface in extract_interfaces(path, &change.base, &clean, "base", &mut unknowns) {
                old_interfaces
                    .entry(interface.name.clone())
                    .or_default()
                    .push(interface);
            }
            if relevant_sources.contains(path) {
                summary.unknowns.extend(unknowns);
            }
        } else if !is_document(&change.path) && !is_manifest(&change.path) {
            summary.unknowns.push(format!(
                "{}: documentation checks support Rust public functions only",
                change.path
            ));
        }
    }
    let doc_count = documents.len();
    for document in documents {
        let path = document.path;
        let source_links = document.links;
        for example in document.examples {
            let line = example.line;
            let cleaned = &example.clean;
            for captures in CALL.captures_iter(cleaned) {
                let name = captures[2].to_string();
                // Matching strings/comments are excluded by lexical_code. Calls
                // inside declarations and qualified method calls are not evidence.
                let Some(whole) = captures.get(0) else {
                    continue;
                };
                let preceding = cleaned[..whole.start()].trim_end();
                if preceding.ends_with('.') || preceding.ends_with("fn") {
                    continue;
                }
                let doc_line = line
                    + cleaned[..whole.start()]
                        .bytes()
                        .filter(|&b| b == b'\n')
                        .count();
                let text = example
                    .original
                    .lines()
                    .nth(doc_line - line)
                    .unwrap_or_default();
                let documentation = evidence(path, doc_line, "current", text);
                let qualified = &captures[1];
                if qualified.is_empty()
                    && (example.wildcard_import || example.shadowed.contains(&name))
                {
                    record_unknown(
                        &mut summary,
                        name,
                        documentation,
                        None,
                        "An example-local declaration or import may shadow this name; source identity is unknown.",
                    );
                    continue;
                }
                let candidates = interfaces.get(&name).map(Vec::as_slice).unwrap_or_default();
                let previous = old_interfaces
                    .get(&name)
                    .map(Vec::as_slice)
                    .unwrap_or_default();
                let associated: Vec<_> = candidates
                    .iter()
                    .filter(|api| associated_source(&api.evidence.path, qualified, &source_links))
                    .collect();
                let old_associated: Vec<_> = previous
                    .iter()
                    .filter(|api| associated_source(&api.evidence.path, qualified, &source_links))
                    .collect();
                if associated.len() == 1 {
                    let api = associated[0];
                    // A nested invocation cannot be safely counted by this pattern.
                    // Only top-level calls with a visibly closed argument list count.
                    let call_arity = argument_count(&captures[3]);
                    match (api.arity, call_arity) {
                        (Some(expected), Some(actual)) if expected != actual => {
                            record_drift(&mut summary, &name, documentation, api.evidence.clone(),
                                format!("Example calls `{name}` with {actual} arguments; the current public declaration requires {expected}."),
                                "Update the example arguments or the public function contract.");
                        }
                        (Some(_), Some(_)) => summary.checks.push(DocsCheck {
                            status: "consistent".into(), symbol: name, documentation,
                            source: Some(api.evidence.clone()),
                            reason: "Visible fixed argument count agrees; types and behavior were not evaluated.".into(),
                        }),
                        _ => record_unknown(&mut summary, name, documentation, Some(api.evidence.clone()),
                            "Unsupported signature or argument syntax; argument compatibility is unknown."),
                    }
                } else if associated.is_empty() && old_associated.len() == 1 {
                    let old = old_associated[0];
                    let changed = changes.iter().find(|change| {
                        change.old_path.as_deref().unwrap_or(&change.path) == old.evidence.path
                    });
                    let Some(change) = changed else { continue };
                    // A move, re-export, macro, private function or another matching
                    // declaration leaves identity unresolved. Do not use repository
                    // absence as proof: require the exact changed file as evidence.
                    // Changes already override the current inventory, so this
                    // precomputed union also covers the changed file's contents.
                    let elsewhere = current_identifiers.contains(&name);
                    if !elsewhere && !opaque_exports {
                        let current_path = change.path.clone();
                        let reason = format!(
                            "Example still calls `{name}`, declared at {}:{} in base; the changed file `{current_path}` {} and no visible replacement establishes compatibility.",
                            old.evidence.path,
                            old.evidence.line,
                            if change.content.is_none() {
                                "was removed"
                            } else {
                                "no longer declares that interface"
                            }
                        );
                        record_drift(
                            &mut summary,
                            &name,
                            documentation,
                            old.evidence.clone(),
                            reason,
                            "Update the example to the replacement interface, or restore the documented public interface. Confirm any re-export outside the supplied inventory.",
                        );
                    } else {
                        record_unknown(
                            &mut summary,
                            name,
                            documentation,
                            Some(old.evidence.clone()),
                            "The previous interface has moved, changed visibility, has another visible reference or opaque exports; replacement/re-export compatibility is unknown.",
                        );
                    }
                } else {
                    record_unknown(
                        &mut summary,
                        name,
                        documentation,
                        None,
                        "No unique source association and supported public declaration establishes this example's interface; external or excluded APIs remain unknown.",
                    );
                }
            }
        }
    }
    if doc_count == 0 {
        summary.unknowns.push("No relevant current Markdown documentation was supplied; documentation coverage is unknown.".into());
    }
    if doc_count > 0 && summary.checks.is_empty() {
        summary.unknowns.push("No supported interface calls were established in the supplied examples; documentation coverage is unknown.".into());
    }
    summary.unknowns.sort();
    summary.unknowns.dedup();
    summary
}

fn is_document(path: &str) -> bool {
    path.ends_with(".md") || path.ends_with(".mdx") || path.ends_with(".markdown")
}

fn is_manifest(path: &str) -> bool {
    path.ends_with("Cargo.toml") || path.ends_with("Cargo.lock")
}

fn evidence(path: &str, line: usize, revision: &str, text: &str) -> DocsEvidence {
    DocsEvidence {
        path: path.into(),
        line,
        revision: revision.into(),
        text: redact(text, &mut Redactions::default()),
    }
}

fn extract_interfaces(
    path: &str,
    content: &str,
    clean: &str,
    revision: &str,
    unknowns: &mut Vec<String>,
) -> Vec<Interface> {
    let original: Vec<_> = content.lines().collect();
    let mut depth: i64 = 0;
    let mut conditional = false;
    let mut attribute_depth = 0i64;
    let (conditional_lines, crate_conditional) = conditional_attributes(clean);
    let mut out = Vec::new();
    for (index, line) in clean.lines().enumerate() {
        if depth == 0 {
            let attribute = attribute_depth > 0 || line.trim_start().starts_with("#[");
            if attribute {
                attribute_depth += line.bytes().filter(|&b| b == b'[').count() as i64;
                attribute_depth -= line.bytes().filter(|&b| b == b']').count() as i64;
            }
            if conditional_lines.contains(&index) {
                conditional = true;
            }
            if let Some(captures) = DECLARATION.captures(line) {
                if conditional || crate_conditional {
                    unknowns.push(format!("{path}:{}: conditional public interface requires build configuration evidence", index + 1));
                } else {
                    let params = &captures[2];
                    let arity = if params.trim().is_empty() {
                        Some(0)
                    } else {
                        let params: Vec<_> = params
                            .split(',')
                            .filter(|param| !param.trim().is_empty())
                            .collect();
                        params
                            .iter()
                            .all(|param| SIMPLE_PARAMETER.is_match(param))
                            .then_some(params.len())
                    };
                    out.push(Interface {
                        name: captures[1].into(),
                        arity,
                        evidence: evidence(
                            path,
                            index + 1,
                            revision,
                            original.get(index).copied().unwrap_or_default(),
                        ),
                    });
                }
                conditional = false;
            } else if line.starts_with("pub ") && line.contains("fn ") {
                unknowns.push(format!(
                    "{path}:{}: multiline or generic public signature is unsupported",
                    index + 1
                ));
                conditional = false;
            } else if !line.trim().is_empty() && !attribute && !line.trim_start().starts_with('#') {
                conditional = false;
            }
        }
        depth += line.bytes().filter(|&b| b == b'{').count() as i64;
        depth -= line.bytes().filter(|&b| b == b'}').count() as i64;
    }
    out
}

/// Distinguish cfg from cfg_attr, including multiline and nested payloads.
/// Benign cfg_attr features/no_std/doc attributes do not remove declarations;
/// a cfg payload (even nested in cfg_attr) can and remains unknown.
fn conditional_attributes(clean: &str) -> (BTreeSet<usize>, bool) {
    let lines: Vec<_> = clean.lines().collect();
    let mut conditional_lines = BTreeSet::new();
    let mut crate_conditional = false;
    let mut scope_depth = 0i64;
    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim_start();
        let attribute = trimmed
            .strip_prefix("#![")
            .map(|body| (true, body))
            .or_else(|| trimmed.strip_prefix("#[").map(|body| (false, body)));
        if scope_depth == 0
            && let Some((inner, first)) = attribute
        {
            let mut body = String::new();
            let mut depth = 1i64;
            let mut complete = false;
            'attribute: for fragment in
                std::iter::once(first).chain(lines[index + 1..].iter().copied())
            {
                for ch in fragment.chars() {
                    if ch == '[' {
                        depth += 1;
                    }
                    if ch == ']' {
                        depth -= 1;
                    }
                    if depth == 0 {
                        complete = true;
                        break 'attribute;
                    }
                    body.push(ch);
                }
                body.push('\n');
            }
            if cfg_attribute_may_disable(&body, complete) {
                if inner {
                    crate_conditional = true;
                } else {
                    conditional_lines.insert(index);
                }
            }
        }
        scope_depth += line.bytes().filter(|&byte| byte == b'{').count() as i64;
        scope_depth -= line.bytes().filter(|&byte| byte == b'}').count() as i64;
    }
    (conditional_lines, crate_conditional)
}

fn cfg_attribute_may_disable(body: &str, complete: bool) -> bool {
    cfg_meta_may_disable(body, complete, 0)
}

fn cfg_meta_may_disable(body: &str, complete: bool, nesting: usize) -> bool {
    if nesting >= 32 {
        return true;
    }
    let body = body.trim();
    let end = body
        .find(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .unwrap_or(body.len());
    match &body[..end] {
        "cfg" => true,
        "cfg_attr" => {
            if !complete {
                return true;
            }
            let Some(args) = body[end..]
                .trim()
                .strip_prefix('(')
                .and_then(|args| args.strip_suffix(')'))
            else {
                return true;
            };
            let mut depth = 0i64;
            let mut parts = Vec::new();
            let mut start = 0;
            for (offset, ch) in args.char_indices() {
                match ch {
                    '(' | '[' | '{' => depth += 1,
                    ')' | ']' | '}' => depth -= 1,
                    ',' if depth == 0 => {
                        parts.push(&args[start..offset]);
                        start = offset + 1;
                    }
                    _ => {}
                }
                if depth < 0 {
                    return true;
                }
            }
            if depth != 0 {
                return true;
            }
            parts.push(&args[start..]);
            if parts.len() < 2 || parts[0].trim().is_empty() {
                return true;
            }
            let payloads: Vec<_> = parts[1..]
                .iter()
                .filter(|part| !part.trim().is_empty())
                .collect();
            payloads.is_empty()
                || payloads
                    .iter()
                    .any(|payload| cfg_meta_may_disable(payload, true, nesting + 1))
        }
        _ => false,
    }
}

/// Blank comments/string bodies while preserving byte positions and newlines.
/// This prevents fake declarations or calls in prose/string literals.
fn lexical_code(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut clean = bytes.to_vec();
    let mut index = 0;
    while index < bytes.len() {
        let start = index;
        if bytes[index..].starts_with(b"//") {
            while index < bytes.len() && bytes[index] != b'\n' {
                index += 1;
            }
        } else if bytes[index..].starts_with(b"/*") {
            index += 2;
            let mut depth = 1;
            while index < bytes.len() && depth > 0 {
                if bytes[index..].starts_with(b"/*") {
                    depth += 1;
                    index += 2;
                } else if bytes[index..].starts_with(b"*/") {
                    depth -= 1;
                    index += 2;
                } else {
                    index += 1;
                }
            }
        } else if bytes[index] == b'"' {
            // Include Rust raw-string hashes in the closing delimiter. A raw
            // opener is directly preceded by r followed by zero or more #s.
            let mut prefix = index;
            while prefix > 0 && bytes[prefix - 1] == b'#' {
                prefix -= 1;
            }
            let raw = prefix > 0 && bytes[prefix - 1] == b'r';
            let hashes = index - prefix;
            index += 1;
            while index < bytes.len() {
                if bytes[index] == b'"'
                    && (!raw
                        || bytes
                            .get(index + 1..index + 1 + hashes)
                            .is_some_and(|part| part.iter().all(|&b| b == b'#')))
                {
                    index += 1 + if raw { hashes } else { 0 };
                    break;
                }
                if !raw && bytes[index] == b'\\' {
                    index = (index + 2).min(bytes.len());
                } else {
                    index += 1;
                }
            }
            // Retain a nonblank marker, so a literal argument counts as one.
            clean[start] = b'0';
            for byte in &mut clean[start + 1..index] {
                if *byte != b'\n' {
                    *byte = b' ';
                }
            }
            continue;
        } else if bytes[index] == b'\'' && bytes.get(index + 2) == Some(&b'\'') {
            index += 3;
            clean[start] = b'0';
            clean[start + 1] = b' ';
            clean[start + 2] = b' ';
            continue;
        } else if bytes[index..].starts_with(b"'\\") && bytes.get(index + 3) == Some(&b'\'') {
            index += 4;
            clean[start] = b'0';
            clean[start + 1..index].fill(b' ');
            continue;
        } else {
            index += 1;
            continue;
        }
        for byte in &mut clean[start..index] {
            if *byte != b'\n' {
                *byte = b' ';
            }
        }
    }
    // Non-ASCII bytes outside comments/strings are unchanged. Within a
    // removed literal all bytes become ASCII spaces, maintaining valid UTF-8.
    String::from_utf8(clean).expect("lexical blanking preserves UTF-8")
}

fn argument_count(arguments: &str) -> Option<usize> {
    if arguments.trim().is_empty() {
        return Some(0);
    }
    // Only simple positional expressions. Complex syntax has no defensible
    // count in this deliberately small recognizer.
    if arguments
        .chars()
        .any(|ch| matches!(ch, '[' | ']' | '{' | '}' | '<' | '>' | '|' | ';' | '='))
    {
        return None;
    }
    let args: Vec<_> = arguments.split(',').collect();
    if args[..args.len().saturating_sub(1)]
        .iter()
        .any(|arg| arg.trim().is_empty())
    {
        return None;
    }
    Some(args.iter().filter(|arg| !arg.trim().is_empty()).count())
}

fn source_links(doc_path: &str, content: &str) -> BTreeSet<String> {
    LINK.captures_iter(content)
        .chain(SOURCE_REFERENCE.captures_iter(content))
        .filter_map(|captures| {
            let target = captures[1].split('#').next()?;
            if target.contains("://") {
                return None;
            }
            let parent = doc_path
                .rsplit_once('/')
                .map(|(parent, _)| parent)
                .unwrap_or_default();
            let joined = if target.starts_with('/') {
                target.trim_start_matches('/').into()
            } else if parent.is_empty() {
                target.into()
            } else {
                format!("{parent}/{target}")
            };
            let mut components = Vec::new();
            for component in joined.split('/') {
                match component {
                    "." | "" => {}
                    ".." => {
                        components.pop()?;
                    }
                    component => components.push(component),
                }
            }
            Some(components.join("/"))
        })
        .collect()
}

fn associated_source(path: &str, qualified: &str, links: &BTreeSet<String>) -> bool {
    if links.contains(path) && qualified.is_empty() {
        return true;
    }
    let Some(module) = path
        .strip_prefix("src/")
        .and_then(|path| path.strip_suffix(".rs"))
    else {
        return false;
    };
    let module = match module {
        "lib" | "main" => String::new(),
        module => module.trim_end_matches("/mod").replace('/', "::"),
    };
    let expected = if module.is_empty() {
        "crate::".into()
    } else {
        format!("crate::{module}::")
    };
    qualified == expected
}

fn add_qualified_sources(qualified: &str, sources: &mut BTreeSet<String>) {
    if qualified == "crate::" {
        sources.extend(["src/lib.rs".into(), "src/main.rs".into()]);
    } else if let Some(module) = qualified
        .strip_prefix("crate::")
        .and_then(|module| module.strip_suffix("::"))
    {
        let module = module.replace("::", "/");
        sources.insert(format!("src/{module}.rs"));
        sources.insert(format!("src/{module}/mod.rs"));
    }
}

/// Tokens from already-cleaned Rust; comments and literals must be removed first.
fn identifiers(clean: &str) -> impl Iterator<Item = &str> {
    clean
        .split(|ch: char| !ch.is_alphanumeric() && ch != '_')
        .filter(|token| !token.is_empty())
}

fn example_shadows(clean: &str) -> (BTreeSet<String>, bool) {
    let mut shadowed = BTreeSet::new();
    let mut wildcard_import = false;
    for import in IMPORT.find_iter(clean) {
        wildcard_import |= import.as_str().contains('*');
        shadowed.extend(identifiers(import.as_str()).map(str::to_owned));
    }
    for line in clean.lines() {
        let line = line.trim_start().trim_start_matches('#').trim_start();
        let declaration = line.split('=').next().unwrap_or(line);
        let tokens: Vec<_> = identifiers(declaration).collect();
        if tokens.first() == Some(&"let") || tokens.contains(&"fn") {
            shadowed.extend(tokens.into_iter().map(str::to_owned));
        }
    }
    (shadowed, wildcard_import)
}

fn has_opaque_exports(clean: &str) -> bool {
    clean.lines().any(|line| {
        let trimmed = line.trim_start();
        (trimmed.starts_with("pub use ") && trimmed.contains('*'))
            || trimmed.starts_with("include!")
            || (!line.starts_with(char::is_whitespace)
                && trimmed.split_once('!').is_some_and(|(name, _)| {
                    !name.is_empty()
                        && name
                            .chars()
                            .all(|ch| ch.is_alphanumeric() || ch == '_' || ch == ':')
                }))
    })
}

fn examples(path: &str, content: &str, unknowns: &mut Vec<String>) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut fence: Option<(char, usize, bool, usize, String)> = None;
    for (index, line) in content.lines().enumerate() {
        let trimmed = line.trim_start();
        if let Some((marker, count, rust, start, body)) = &mut fence {
            if trimmed.starts_with(&marker.to_string().repeat(*count))
                && trimmed.trim_matches(*marker).trim().is_empty()
            {
                if *rust {
                    out.push((*start, std::mem::take(body)));
                }
                fence = None;
            } else {
                body.push_str(line);
                body.push('\n');
            }
        } else if trimmed.starts_with("```") || trimmed.starts_with("~~~") {
            let marker = trimmed.chars().next().unwrap_or('`');
            let count = trimmed.chars().take_while(|&ch| ch == marker).count();
            let language = trimmed[count..]
                .split(',')
                .next()
                .unwrap_or_default()
                .trim();
            let rust = language == "rust" || language == "rs";
            if !rust {
                unknowns.push(format!(
                    "{path}:{}: fenced example language `{language}` is unsupported",
                    index + 1
                ));
            }
            fence = Some((marker, count, rust, index + 2, String::new()));
        }
    }
    if fence.is_some() {
        unknowns.push(format!(
            "{path}: unterminated fenced example cannot establish drift"
        ));
    }
    if out.is_empty() {
        unknowns.push(format!("{path}: no complete Rust fenced examples; prose, inline examples and other documentation semantics remain unknown"));
    }
    out
}

fn record_unknown(
    summary: &mut DocsDriftSummary,
    name: String,
    documentation: DocsEvidence,
    source: Option<DocsEvidence>,
    reason: &str,
) {
    summary.unknowns.push(format!(
        "{}:{}: {reason}",
        documentation.path, documentation.line
    ));
    summary.checks.push(DocsCheck {
        status: "unknown".into(),
        symbol: name,
        documentation,
        source,
        reason: reason.into(),
    });
}

fn record_drift(
    summary: &mut DocsDriftSummary,
    name: &str,
    documentation: DocsEvidence,
    source: DocsEvidence,
    reason: String,
    fix: &str,
) {
    let joined = format!(
        "{}:{} ({}): {}\n{}:{} ({}): {}",
        documentation.path,
        documentation.line,
        documentation.revision,
        documentation.text,
        source.path,
        source.line,
        source.revision,
        source.text
    );
    summary.findings.push(Finding {
        file: documentation.path.clone(),
        line: documentation.line,
        dimension: Dimension::Correctness,
        probability: 1.0,
        location_confidence: 1.0,
        mechanism: "docsInterfaceDrift".into(),
        mechanism_confidence: 1.0,
        severity: 1.0,
        severity_confidence: 1.0,
        action: Action::Comment,
        evidence: joined,
        title: Some(format!("Documentation example disagrees with `{name}`")),
        why: Some(reason.clone()),
        fix: Some(fix.into()),
        test: Some(
            "Compile the corrected example against the intended crate and build configuration."
                .into(),
        ),
        ..Finding::default()
    });
    summary.checks.push(DocsCheck {
        status: "drift".into(),
        symbol: name.into(),
        documentation,
        source: Some(source),
        reason,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(path: &str, content: &str) -> SourceFile {
        SourceFile {
            path: path.into(),
            content: content.into(),
        }
    }
    fn change(path: &str, base: &str, content: Option<&str>) -> RepositoryChange {
        RepositoryChange {
            path: path.into(),
            old_path: None,
            base: base.into(),
            content: content.map(str::to_string),
        }
    }
    fn docs(example: &str) -> String {
        format!("# API\n[source](src/lib.rs)\n```rust\n{example}\n```\n")
    }

    #[test]
    fn documentation_only_mismatch_has_exact_current_locations() {
        let document = docs("answer();");
        let summary = assess(
            &[change("README.md", "", Some(&document))],
            &[source("src/lib.rs", "// API\npub fn answer(value: u32) {}")],
        );
        assert_eq!(summary.findings.len(), 1);
        assert_eq!(summary.findings[0].line, 4);
        assert_eq!(summary.checks[0].source.as_ref().unwrap().line, 2);
        assert!(
            summary.findings[0]
                .evidence
                .contains("src/lib.rs:2 (current)")
        );
        assert_eq!(summary.findings[0].action, Action::Comment);
        assert!(summary.findings[0].fingerprint.is_empty());
    }

    #[test]
    fn explicit_inline_source_reference_establishes_a_root_file_identity() {
        let document = "See `lib.rs`.\n```rust\nf();\n```\n";
        let summary = assess(
            &[change("README.md", "", Some(document))],
            &[source("lib.rs", "pub fn f(x: i32) {}")],
        );
        assert_eq!(summary.findings.len(), 1);
        assert_eq!(summary.findings[0].file, "README.md");
        assert_eq!(summary.findings[0].line, 3);
        assert_eq!(summary.checks[0].source.as_ref().unwrap().path, "lib.rs");
    }

    #[test]
    fn unchanged_docs_are_checked_when_public_interface_changes() {
        let summary = assess(
            &[change(
                "src/lib.rs",
                "pub fn answer() {}",
                Some("pub fn answer(value: u32) {}"),
            )],
            &[source("README.md", &docs("crate::answer();"))],
        );
        assert_eq!(summary.findings.len(), 1);
        assert!(
            summary.findings[0]
                .why
                .as_ref()
                .unwrap()
                .contains("requires 1")
        );
    }

    #[test]
    fn removed_and_renamed_symbols_require_base_evidence() {
        for content in [None, Some("pub fn replacement() {}")] {
            let summary = assess(
                &[change("src/lib.rs", "\npub fn old_api() {}", content)],
                &[source("README.md", &docs("old_api();"))],
            );
            assert_eq!(summary.findings.len(), 1);
            assert_eq!(summary.checks[0].source.as_ref().unwrap().revision, "base");
            assert!(summary.findings[0].evidence.contains("src/lib.rs:2 (base)"));
        }
        let summary = assess(
            &[change("src/lib.rs", "", None)],
            &[source("README.md", &docs("old_api();"))],
        );
        assert!(summary.findings.is_empty());
        assert!(!summary.unknowns.is_empty());
    }

    #[test]
    fn repeated_removed_calls_reuse_exact_lexical_reference_evidence() {
        let document = docs(&"old_api();\n".repeat(128));
        let changed = change("src/lib.rs", "pub fn old_api() {}", None);
        let unrelated = "// old_api();\nconst TEXT: &str = r#\"old_api()\"#;\npub fn old_api_suffix() {}\n/* pub use api::*; */\nconst MACRO_TEXT: &str = \"include!(x)\";";
        let summary = assess(
            std::slice::from_ref(&changed),
            &[
                source("README.md", &document),
                source("src/other.rs", unrelated),
            ],
        );
        assert_eq!(summary.findings.len(), 128);
        assert_eq!(summary.findings[0].line, 4);
        assert_eq!(summary.findings[127].line, 131);
        assert!(summary.unknowns.is_empty());
        for replacement in [
            "fn caller() { old_api(); }",
            "pub use api::*;",
            "include!(\"generated.rs\");",
            "generate_api!();",
        ] {
            let summary = assess(
                std::slice::from_ref(&changed),
                &[
                    source("README.md", &document),
                    source("src/other.rs", replacement),
                ],
            );
            assert!(summary.findings.is_empty(), "{replacement}");
            assert_eq!(summary.checks.len(), 128);
            assert!(summary.checks.iter().all(|check| check.status == "unknown"));
        }
    }

    #[test]
    fn unsupported_declaration_diagnostics_follow_document_source_identity() {
        let unrelated = "pub fn generic<T>(value: T) {}\npub fn multiline(\nvalue: u32\n) {}\n#[cfg(feature = \"optional\")]\npub fn conditional() {}";
        for document in [docs("answer();"), "```rust\ncrate::answer();\n```".into()] {
            let summary = assess(
                &[change("README.md", "", Some(&document))],
                &[
                    source("src/lib.rs", "pub fn answer(value: u32) {}"),
                    source("src/unrelated.rs", unrelated),
                ],
            );
            assert_eq!(summary.findings.len(), 1);
            assert!(summary.unknowns.is_empty(), "{:?}", summary.unknowns);
        }
        // Crate-qualified module paths retain relevant unsupported diagnostics
        // for both supported physical module layouts.
        for path in ["src/api.rs", "src/api/mod.rs"] {
            let document = "```rust\ncrate::api::generic();\n```";
            let summary = assess(
                &[change("README.md", "", Some(document))],
                &[source(path, "pub fn generic<T>(value: T) {}")],
            );
            assert!(summary.findings.is_empty());
            assert!(
                summary
                    .unknowns
                    .iter()
                    .any(|unknown| unknown.starts_with(path))
            );
            assert_eq!(summary.checks[0].status, "unknown");
        }
    }

    #[test]
    fn unsupported_base_diagnostics_are_scoped_but_unresolved_calls_stay_unknown() {
        let summary = assess(
            &[
                change("README.md", "", Some(&docs("answer();"))),
                change("src/unrelated.rs", "pub fn generic<T>(value: T) {}", None),
            ],
            &[source("src/lib.rs", "pub fn answer() {}")],
        );
        assert_eq!(summary.checks[0].status, "consistent");
        assert!(summary.unknowns.is_empty());
        let summary = assess(
            &[change("README.md", "", Some("```rust\ngeneric();\n```"))],
            &[source("src/unrelated.rs", "pub fn generic<T>(value: T) {}")],
        );
        assert_eq!(summary.checks[0].status, "unknown");
        assert!(!summary.unknowns.is_empty());
    }

    #[test]
    fn moved_reexported_or_ambiguous_interfaces_abstain() {
        let mut moved = change(
            "src/new.rs",
            "pub fn old_api() {}",
            Some("pub fn old_api() {}"),
        );
        moved.old_path = Some("src/lib.rs".into());
        let summary = assess(&[moved], &[source("README.md", &docs("old_api();"))]);
        assert!(summary.findings.is_empty());
        assert!(summary.checks.iter().any(|check| check.status == "unknown"));
        let summary = assess(
            &[change(
                "src/lib.rs",
                "pub fn old_api() {}",
                Some("pub use other::old_api;"),
            )],
            &[source("README.md", &docs("old_api();"))],
        );
        assert!(summary.findings.is_empty());
        let summary = assess(
            &[change(
                "src/lib.rs",
                "pub fn old_api() {}",
                Some("pub use other::*;"),
            )],
            &[source("README.md", &docs("old_api();"))],
        );
        assert!(summary.findings.is_empty());
        assert!(!summary.unknowns.is_empty());
        let document = "```rust\nanswer();\n```";
        let summary = assess(
            &[change("README.md", "", Some(document))],
            &[source("src/lib.rs", "pub fn answer(x: u32) {}")],
        );
        assert!(summary.findings.is_empty());
        assert!(!summary.unknowns.is_empty());
    }

    #[test]
    fn matching_examples_comments_strings_and_complex_syntax_do_not_invent_drift() {
        let document =
            docs("answer(\"a,b\");\n// answer();\nlet text = \"answer()\";\nanswer(nested());");
        let summary = assess(
            &[change("README.md", "", Some(&document))],
            &[source("src/lib.rs", "pub fn answer(value: &str) {}")],
        );
        assert!(summary.findings.is_empty());
        assert!(
            summary
                .checks
                .iter()
                .any(|check| check.status == "consistent")
        );
        let document = docs("answer([1, 2]);");
        let summary = assess(
            &[change("README.md", "", Some(&document))],
            &[source("src/lib.rs", "pub fn answer(value: &str) {}")],
        );
        assert!(summary.findings.is_empty());
        assert!(summary.checks.iter().any(|check| check.status == "unknown"));
    }

    #[test]
    fn unsupported_conditional_and_incomplete_evidence_is_unknown() {
        for (path, code) in [
            (
                "src/lib.rs",
                "#[cfg(feature = \"special\")]\npub fn answer(x: u32) {}",
            ),
            ("src/lib.rs", "pub fn answer<T>(x: T) {}"),
            ("src/lib.rs", "pub fn answer(\n x: u32\n) {}"),
            ("src/api.py", "def answer(x): pass"),
        ] {
            let summary = assess(
                &[change(path, "", Some(code))],
                &[source("README.md", &docs("answer();"))],
            );
            assert!(summary.findings.is_empty(), "{code}");
            assert!(!summary.unknowns.is_empty());
        }
        let summary = assess(
            &[change("README.md", "", Some("```rust\ncrate::answer();"))],
            &[source("src/lib.rs", "pub fn answer(x: u32) {}")],
        );
        assert!(summary.findings.is_empty());
        assert!(
            summary
                .unknowns
                .iter()
                .any(|unknown| unknown.contains("unterminated"))
        );
    }

    #[test]
    fn relative_links_module_paths_deletions_and_secret_redaction() {
        let doc = "[source](../src/api.rs)\n```rust\nanswer(); // AKIAIOSFODNN7EXAMPLE\n```";
        let summary = assess(
            &[change("docs/api.md", "", Some(doc))],
            &[source("src/api.rs", "pub fn answer(x: u32) {}")],
        );
        assert_eq!(summary.findings.len(), 1);
        assert!(
            !serde_json::to_string(&summary)
                .unwrap()
                .contains("AKIAIOSFODNN7EXAMPLE")
        );
        let doc = "```rust\ncrate::api::answer();\n```";
        let summary = assess(
            &[change("docs/api.md", "", Some(doc))],
            &[source("src/api.rs", "pub fn answer(x: u32) {}")],
        );
        assert_eq!(summary.findings.len(), 1);
        let summary = assess(
            &[change("README.md", &docs("answer();"), None)],
            &[
                source("README.md", &docs("answer();")),
                source("src/lib.rs", "pub fn answer(x: u32) {}"),
            ],
        );
        assert!(summary.findings.is_empty());
    }

    #[test]
    fn nested_and_commented_declarations_are_not_public_interface_evidence() {
        let code = "/*\npub fn answer(x: u32) {}\n*/\nfn outer() {\npub fn answer(x: u32) {}\n}\nconst EXAMPLE: &str = r#\"\npub fn answer(x: u32) {}\n\"#;";
        let summary = assess(
            &[change("README.md", "", Some(&docs("answer();")))],
            &[source("src/lib.rs", code)],
        );
        assert!(summary.findings.is_empty());
        assert!(!summary.unknowns.is_empty());
    }

    #[test]
    fn multiline_cfg_crate_cfg_and_example_bindings_abstain() {
        for code in [
            "#[cfg(\nfeature = \"special\"\n)]\npub fn answer(x: u32) {}",
            "#![cfg(feature = \"special\")]\npub fn answer(x: u32) {}",
        ] {
            let summary = assess(
                &[change("README.md", "", Some(&docs("answer();")))],
                &[source("src/lib.rs", code)],
            );
            assert!(summary.findings.is_empty());
            assert!(!summary.unknowns.is_empty());
        }
        for example in [
            "use external::answer;\nanswer();",
            "fn answer() {}\nanswer();",
            "let answer = || {};\nanswer();",
            "async fn answer() {}\nanswer();",
            "use external::{\nanswer\n};\nanswer();",
            "use external::*;\nanswer();",
        ] {
            let summary = assess(
                &[change("README.md", "", Some(&docs(example)))],
                &[source("src/lib.rs", "pub fn answer(x: u32) {}")],
            );
            assert!(summary.findings.is_empty());
            assert!(
                summary
                    .unknowns
                    .iter()
                    .any(|unknown| unknown.contains("shadow"))
            );
        }
    }

    #[test]
    fn benign_cfg_attr_does_not_disable_public_interface_comparisons() {
        for attribute in [
            "#![cfg_attr(docsrs, feature(doc_cfg))]",
            "#![cfg_attr(not(feature = \"std\"), no_std)]",
            "#![cfg_attr(\n docsrs,\n feature(doc_cfg)\n)]",
            "#[cfg_attr(docsrs, doc(cfg(feature = \"std\")))]",
            "#[cfg_attr(docsrs, doc = \"cfg(hidden)\")]",
        ] {
            let code = format!("{attribute}\npub fn answer(x: u32) {{}}");
            let summary = assess(
                &[change("README.md", "", Some(&docs("answer();")))],
                &[source("src/lib.rs", &code)],
            );
            assert_eq!(
                summary.findings.len(),
                1,
                "{attribute}: {:?}",
                summary.unknowns
            );
        }
    }

    #[test]
    fn true_cfg_and_cfg_attr_that_can_remove_interfaces_stay_unknown() {
        for attribute in [
            "#![cfg(feature = \"std\")]",
            "#![cfg (feature = \"std\")]",
            "#![cfg_attr(docsrs, cfg(feature = \"std\"))]",
            "#![cfg_attr(\n docsrs,\n cfg(feature = \"std\")\n)]",
            "#[cfg_attr(docsrs, cfg(feature = \"std\"))]",
            "#[cfg_attr(docsrs, cfg_attr(unix, cfg(feature = \"std\")))]",
        ] {
            let code = format!("{attribute}\npub fn answer(x: u32) {{}}");
            let summary = assess(
                &[change("README.md", "", Some(&docs("answer();")))],
                &[source("src/lib.rs", &code)],
            );
            assert!(summary.findings.is_empty(), "{attribute}");
            assert!(
                summary
                    .unknowns
                    .iter()
                    .any(|unknown| unknown.contains("conditional public interface")),
                "{attribute}"
            );
        }
    }
}
