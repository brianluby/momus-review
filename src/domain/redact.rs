//! Best-effort credential redaction for text and JSON string values.
//!
//! The System One client uses these helpers on outgoing state unless redaction
//! is disabled. Local auxiliary checks and publication adapters also call them
//! explicitly. The functions perform no I/O and sanitize only the values their
//! caller supplies; questions, object keys, and unrelated local artifacts are
//! outside an outgoing-state pass.
//!
//! Each matched value is replaced by a typed placeholder such as
//! `<redacted:aws-access-key>`, retaining evidence of a credential-like literal
//! for hardcoded-secret review. This does not guarantee a finding. Replacements
//! never add or remove a newline, so line-based hunk starts and region positions remain
//! usable. Byte lengths and columns can change and must not be reused.
//!
//! Detection uses provider patterns, credential assignments, PEM blocks, and
//! an entropy heuristic. It deliberately leaves many references, templates,
//! and benign-looking values alone, and can both miss secrets and redact
//! nonsecrets. Successful redaction or an empty match log is no secrecy
//! guarantee. Retain separate handling for inputs whose confidentiality must
//! be assured before sending or publishing them.
//!
//! Outgoing-state redaction changes request state. Ordinary local source
//! evidence, fingerprints, and saved reports can retain original text unless a caller
//! explicitly sanitizes an export. Matched plaintext also lives temporarily in
//! [`Redactions`]; consuming or dropping it does not securely erase memory.

use std::collections::{BTreeMap, HashSet};
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::LazyLock;

use regex::{Captures, Regex};
use serde_json::Value;

/// The placeholder prefix; values already carrying it are never re-redacted.
const PLACEHOLDER_PREFIX: &str = "<redacted:";

/// The placeholder that replaces a secret matched by `rule`.
///
/// This only formats `<redacted:{rule}>`; it neither checks that `rule` is a
/// known detector nor inspects or sanitizes the supplied label. Detection uses
/// fixed rule names. Callers constructing their own placeholders must provide
/// a suitable label themselves.
pub fn placeholder(rule: &str) -> String {
    format!("{PLACEHOLDER_PREFIX}{rule}>")
}

/// One detection rule. Group 1 holds the secret; the rest of the match (key
/// names, quotes, URL scheme) is kept so the model still sees the shape of
/// the code.
struct Rule {
    name: &'static str,
    regex: Regex,
    guard: Guard,
}

/// Which matched values a rule lets through as not-a-secret.
#[derive(Clone, Copy)]
enum Guard {
    /// A provider prefix is proof enough: always redact.
    None,
    /// Skip references and templates (`${DB_PASS}`, `<password>`) only: a
    /// URL password is a secret whatever it looks like, even `pw`.
    Reference,
    /// Also skip identifiers, UI labels, and the other shapes in
    /// `is_benign_value`: a secret-looking *name* is weak evidence.
    Benign,
}

fn rule(name: &'static str, guard: Guard, pattern: &str) -> Rule {
    Rule {
        name,
        regex: Regex::new(pattern).expect("valid redaction regex"),
        guard,
    }
}

/// A provider token rule: the distinctive prefix identifies the secret.
fn token(name: &'static str, pattern: &str) -> Rule {
    rule(name, Guard::None, pattern)
}

/// A rule keyed on a secret-looking name: the value may be an identifier,
/// label, or reference rather than a secret.
fn named(name: &'static str, pattern: &str) -> Rule {
    rule(name, Guard::Benign, pattern)
}

/// Token rules, most specific first: a value taken by an earlier rule is a
/// placeholder by the time later rules run. No pattern may match `\n`.
static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    vec![
        token(
            "aws-access-key",
            r"\b((?:AKIA|ASIA|ABIA|ACCA)[0-9A-Z]{16})\b",
        ),
        token("github-token", r"\b(gh[pousr]_[A-Za-z0-9]{36,255})\b"),
        token("github-token", r"\b(github_pat_[A-Za-z0-9_]{22,255})\b"),
        token("gitlab-token", r"\b(glpat-[A-Za-z0-9_-]{20,})"),
        token("slack-token", r"\b(xox[baprs]-[A-Za-z0-9-]{10,})"),
        token(
            "slack-webhook",
            r"(https://hooks\.slack\.com/services/[A-Za-z0-9/_-]+)",
        ),
        token(
            "stripe-key",
            r"\b((?:sk|rk)_(?:live|test)_[A-Za-z0-9]{16,})",
        ),
        token("google-api-key", r"\b(AIza[0-9A-Za-z_-]{35})"),
        token("anthropic-key", r"\b(sk-ant-[A-Za-z0-9_-]{20,})"),
        token(
            "openai-key",
            r"\b(sk-(?:proj-|svcacct-|admin-)?[A-Za-z0-9_-]{32,})",
        ),
        token(
            "jwt",
            r"\b(eyJ[A-Za-z0-9_-]{10,}\.eyJ[A-Za-z0-9_-]{10,}\.[A-Za-z0-9_-]{10,})",
        ),
        // `scheme://user:password@host` — only the password is replaced. The
        // authority ends at its *last* `@`, so a password holding an
        // unescaped `@` (`u:p@ss@db`) is replaced whole.
        rule(
            "url-credentials",
            Guard::Reference,
            r#"\b[A-Za-z][A-Za-z0-9+.-]*://[^/\s:@'"`]+:([^\s/?#'"`]*[^@\s/?#'"`])@"#,
        ),
        // `password = "…"`, `"apiKey": '…'`, `client_secret => "…"`: a quoted
        // literal assigned to a secret-looking name (any case). A quoted name
        // must fill its quotes, so an i18n sentence key ending in "password."
        // is not a name.
        named(
            "generic-secret",
            r#"(?i)(?:["'`][A-Za-z0-9_.-]*(?:password|passwd|pwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|credential)[A-Za-z0-9_.-]*["'`]|\b[A-Za-z0-9_.-]*(?:password|passwd|pwd|secret|token|api[_-]?key|access[_-]?key|private[_-]?key|credential)[A-Za-z0-9_.-]*)[ \t]*(?::=|=>|:|=)[ \t]*(?:b|r|f)?["'`]([^"'`\s]{6,})["'`]"#,
        ),
        // Env-file style: `DB_PASSWORD=hunter22` (upper-case name, unquoted
        // value), optionally behind a diff marker or `export`.
        named(
            "generic-secret",
            r"(?m)^[+\- ]?[ \t]*(?:export[ \t]+)?[A-Z0-9_]*(?:PASSWORD|PASSWD|SECRET|TOKEN|API_KEY|APIKEY|ACCESS_KEY|PRIVATE_KEY|CREDENTIALS?)[A-Z0-9_]*[ \t]*=[ \t]*([^\s'`#]{6,})",
        ),
    ]
});

/// Long random-looking quoted literals (entropy fallback): base64/url-safe
/// alphabet, 32+ characters.
static ENTROPY_CANDIDATE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"["'`]([A-Za-z0-9+/_=-]{32,})["'`]"#).expect("valid regex"));

static PEM_BEGIN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"-----BEGIN[A-Z0-9 ]* PRIVATE KEY( BLOCK)?-----").expect("valid regex")
});
static PEM_END: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"-----END[A-Z0-9 ]* PRIVATE KEY( BLOCK)?-----").expect("valid regex")
});
/// What a line inside a PEM block looks like: a base64 run or an RFC 1421
/// header, optionally wrapped as a string literal (`'MIIE…\n' +`).
static PEM_BODY: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"^[ \t]*["'`]?(?:(?:Proc-Type|DEK-Info):[^"'`]*|[A-Za-z0-9+/=]*)(?:\\r)?(?:\\n)?["'`]?[ \t]*[+,]?[ \t]*\r?$"#,
    )
    .expect("valid regex")
});

/// A value that is obviously not a secret: a reference or template, an
/// existing placeholder, a plain identifier (`access_token`, `X-Api-Key`)
/// naming the secret rather than holding it, a UI label (`Password`,
/// translated text), a version range (`"ngx-window-token": "^7.0.0"`), or a
/// public on-chain address.
/// A reference or template standing in for a secret, a mask, or an
/// existing placeholder.
fn is_reference(value: &str) -> bool {
    value.starts_with(PLACEHOLDER_PREFIX)
        || value.starts_with('$')
        || value.starts_with('{')
        || value.starts_with('<')
        || value.contains("process.env")
        || value.contains("${")
        || value.contains("{{")
        || value
            .chars()
            .all(|c| c == '*' || c == 'x' || c == 'X' || c == '.')
}

fn is_benign_value(value: &str) -> bool {
    static IDENTIFIER: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^(?:[a-z]+(?:[_.-][a-z]+)+|[A-Z]+(?:[_.-][A-Z]+)+|[A-Z][a-z]*(?:-[A-Z][a-z]*)+|[A-Z]?[a-z]+|0x[0-9a-fA-F]{40}|[\^~<>=v]*\d+(?:\.[\dx*]+)+(?:[-+][0-9A-Za-z.-]+)?)$",
        )
        .expect("valid regex")
    });
    let translated = !value.is_ascii() && !value.chars().any(|c| c.is_ascii_digit());
    is_reference(value) || value.starts_with('%') || translated || IDENTIFIER.is_match(value)
}

/// Shannon entropy in bits per character.
fn entropy(s: &str) -> f64 {
    let mut counts: BTreeMap<char, usize> = BTreeMap::new();
    for c in s.chars() {
        *counts.entry(c).or_default() += 1;
    }
    let len = s.chars().count() as f64;
    counts
        .values()
        .map(|&n| {
            let p = n as f64 / len;
            -p * p.log2()
        })
        .sum()
}

/// Whether a quoted literal looks like a random key: mixed upper, lower, and
/// digits with high entropy. Pure hex (hashes, colors, test vectors) and
/// path-like strings are left alone; a hex secret assigned to a secret-named
/// variable is caught by `generic-secret` instead.
fn looks_random(value: &str) -> bool {
    let has = |f: fn(&char) -> bool| value.chars().any(|c| f(&c));
    has(char::is_ascii_uppercase)
        && has(char::is_ascii_lowercase)
        && has(char::is_ascii_digit)
        && !value.contains("//")
        // A charset literal (`ABC…xyz0123456789`) is not a key, and the model
        // needs to see it to judge hand-rolled random-string generation.
        && !["abcdef", "ABCDEF", "012345"].iter().any(|run| value.contains(run))
        && entropy(value) >= 4.3
}

/// Accumulator of matched rule names and plaintext values from redaction passes.
///
/// [`redact`] and [`redact_value`] append to this value; reusing it combines
/// passes and preserves duplicate matches. [`RedactionLog::record`] consumes
/// it to retain hash-based counts instead. It owns plaintext until then or
/// until dropped, with no secure erasure. Its derived `Debug` output includes
/// those values, so do not log the accumulator when secrets must stay private.
#[derive(Debug, Default, PartialEq)]
pub struct Redactions(Vec<(&'static str, String)>);

impl Redactions {
    fn push(&mut self, rule: &'static str, secret: &str) {
        self.0.push((rule, secret.to_string()));
    }

    /// Whether no match has been appended to this accumulator.
    ///
    /// This says nothing about secrets that the detectors did not recognize.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Rule names in detection-pass order, including duplicates.
    ///
    /// Detectors run by rule, so this need not be the order of values in the
    /// original text. The returned labels do not expose matched plaintext.
    pub fn rules(&self) -> Vec<&'static str> {
        self.0.iter().map(|(rule, _)| *rule).collect()
    }
}

/// Replaces values recognized by the built-in detectors and appends their matches.
///
/// The returned text has exactly as many `\n` characters as the input. Context
/// surrounding an ordinary match is retained; a PEM block can have its body
/// blanked while retaining its line breaks and diff markers. Columns and byte
/// offsets can change. The original `text` is borrowed and remains untouched.
///
/// `found` is not cleared: use a fresh [`Redactions`] for an individual pass,
/// or deliberately reuse it to accumulate matches. Recognized placeholders and
/// common references are left alone. Detection is best effort, so neither a
/// successful return nor no matches certifies that the text is safe to disclose.
///
/// ```
/// use momus_review::domain::redact::{redact, RedactionLog, Redactions};
///
/// let source = "+const password = \"hunter2hunter2\";\n+next();\n";
/// let mut log = RedactionLog::default();
/// for _ in 0..2 {
///     let mut found = Redactions::default();
///     let output = redact(source, &mut found);
///     assert_eq!(output, "+const password = \"<redacted:generic-secret>\";\n+next();\n");
///     assert_eq!(output.matches('\n').count(), source.matches('\n').count());
///     assert_eq!(found.rules(), vec!["generic-secret"]);
///     let mut second_pass = Redactions::default();
///     assert_eq!(redact(&output, &mut second_pass), output);
///     assert!(second_pass.is_empty());
///     log.record(found);
/// }
/// // Repeated requests with the same matched value contribute one hash entry.
/// assert_eq!(log.summary().get("generic-secret"), Some(&1));
/// ```
pub fn redact(text: &str, found: &mut Redactions) -> String {
    let mut out = redact_pem(text, found);
    for rule in RULES.iter() {
        if !rule.regex.is_match(&out) {
            continue;
        }
        out = rule
            .regex
            .replace_all(&out, |caps: &Captures| replace_group(caps, rule, found))
            .into_owned();
    }
    if ENTROPY_CANDIDATE.is_match(&out) {
        out = ENTROPY_CANDIDATE
            .replace_all(&out, |caps: &Captures| {
                let whole = caps.get(0).expect("match").as_str();
                let value = caps.get(1).expect("group").as_str();
                if is_benign_value(value) || !looks_random(value) {
                    return whole.to_string();
                }
                found.push("high-entropy", value);
                whole.replacen(value, &placeholder("high-entropy"), 1)
            })
            .into_owned();
    }
    out
}

/// Rewrites a rule match with its secret group replaced, keeping the
/// surrounding context (key name, quotes, URL) verbatim.
fn replace_group(caps: &Captures, rule: &Rule, found: &mut Redactions) -> String {
    let whole = caps.get(0).expect("match");
    let Some(secret) = caps.get(1) else {
        return whole.as_str().to_string();
    };
    let skip = match rule.guard {
        Guard::None => false,
        Guard::Reference => is_reference(secret.as_str()),
        Guard::Benign => is_benign_value(secret.as_str()),
    };
    if skip {
        return whole.as_str().to_string();
    }
    found.push(rule.name, secret.as_str());
    let (start, end) = (secret.start() - whole.start(), secret.end() - whole.start());
    let text = whole.as_str();
    format!(
        "{}{}{}",
        &text[..start],
        placeholder(rule.name),
        &text[end..]
    )
}

/// Redacts PEM private-key blocks line by line, so a block inside a diff
/// keeps each line's `+`/`-`/` ` marker and every newline. A block on one
/// line (a string with `\n` escapes) collapses to one placeholder.
///
/// A block runs to its END marker for as long as its lines look like PEM
/// body (`PEM_BODY`), with no length cap. A line that does not ends it: a
/// hunk that cuts a key short stops at the first line of code after it. A
/// BEGIN marker followed by code on its own line (a PEM parser's constant or
/// `starts_with` check) is a reference, not a key, and is left alone.
fn redact_pem(text: &str, found: &mut Redactions) -> String {
    if !PEM_BEGIN.is_match(text) {
        return text.to_string();
    }
    let rule = "private-key";
    let mut out = String::with_capacity(text.len());
    let mut open = false;
    let mut body = String::new();

    for line in text.split_inclusive('\n') {
        let (content, newline) = match line.strip_suffix('\n') {
            Some(content) => (content, "\n"),
            None => (line, ""),
        };
        if !open {
            match PEM_BEGIN.find(content) {
                Some(begin) => {
                    let rest = &content[begin.end()..];
                    if let Some(end) = PEM_END.find(rest) {
                        found.push(rule, &rest[..end.start()]);
                        out.push_str(&content[..begin.start()]);
                        out.push_str(&placeholder(rule));
                        out.push_str(&rest[end.end()..]);
                    } else if PEM_BODY.is_match(rest) {
                        body = rest.to_string();
                        out.push_str(&content[..begin.start()]);
                        out.push_str(&placeholder(rule));
                        open = true;
                    } else {
                        out.push_str(content);
                    }
                }
                None => out.push_str(content),
            }
        } else {
            // Keep a leading diff marker so the patch stays well-formed.
            let marker_len = usize::from(content.starts_with(['+', '-', ' ']));
            let payload = &content[marker_len..];
            if let Some(end) = PEM_END.find(payload) {
                body.push_str(&payload[..end.start()]);
                found.push(rule, &body);
                body.clear();
                out.push_str(&content[..marker_len]);
                out.push_str(&payload[end.end()..]);
                open = false;
            } else if PEM_BODY.is_match(payload) {
                body.push_str(payload);
                out.push_str(&content[..marker_len]);
            } else {
                found.push(rule, &body);
                body.clear();
                out.push_str(content);
                open = false;
            }
        }
        out.push_str(newline);
    }
    if open {
        found.push(rule, &body);
    }
    out
}

/// Recursively redacts JSON string values in place, accumulating matches.
///
/// Arrays and object values are visited; object keys and non-string scalar
/// values are unchanged. The outgoing-state caller uses schema names it owns
/// as keys. A caller with user-controlled or potentially sensitive keys must
/// handle those keys separately. This function inherits [`redact`]'s best-effort
/// detection and newline-preservation contract and does not clear `found`.
///
/// ```
/// use momus_review::domain::redact::{redact_value, Redactions};
/// use serde_json::json;
///
/// let mut state = json!({
///     "context": [{"text": "AKIAIOSFODNN7EXAMPLE"}],
///     "AKIAIOSFODNN7EXAMPLE": "schema key is left untouched",
///     "line": 7,
/// });
/// let mut found = Redactions::default();
/// redact_value(&mut state, &mut found);
/// assert_eq!(state["context"][0]["text"], "<redacted:aws-access-key>");
/// assert_eq!(state["AKIAIOSFODNN7EXAMPLE"], "schema key is left untouched");
/// assert_eq!(state["line"], 7);
/// assert_eq!(found.rules(), vec!["aws-access-key"]);
/// ```
pub fn redact_value(value: &mut Value, found: &mut Redactions) {
    match value {
        Value::String(s) => {
            let redacted = redact(s, found);
            if redacted != *s {
                *s = redacted;
            }
        }
        Value::Array(items) => items.iter_mut().for_each(|v| redact_value(v, found)),
        Value::Object(map) => map.values_mut().for_each(|v| redact_value(v, found)),
        _ => {}
    }
}

/// Per-run diagnostic counts of hash-distinct matched values per rule.
/// The same file is sent many times (one screen per dimension, follow-ups,
/// refinement), so counting occurrences would overstate what was hidden.
///
/// Recorded values are retained only as `DefaultHasher` outputs, with separate
/// sets for each rule. Those hashes are neither cryptographic protection nor
/// stable persisted identities; collisions can undercount distinct values.
/// Counts describe detector matches, not a verified inventory of secrets or
/// proof that an output was safe to transmit. Plaintext is held in
/// [`Redactions`] until [`record`](Self::record) consumes it.
#[derive(Debug, Default)]
pub struct RedactionLog {
    seen: BTreeMap<&'static str, HashSet<u64>>,
}

impl RedactionLog {
    /// Consumes matches and deduplicates their hashes independently per rule.
    ///
    /// Repeated values matched under one rule count once; a value matched under
    /// two rules can count in both. Consuming matches does not securely erase
    /// their former plaintext allocations.
    pub fn record(&mut self, found: Redactions) {
        for (rule, secret) in found.0 {
            let mut hasher = DefaultHasher::new();
            secret.hash(&mut hasher);
            self.seen.entry(rule).or_default().insert(hasher.finish());
        }
    }

    /// Sorted rule names mapped to the number of distinct stored hashes.
    ///
    /// Returns an owned snapshot without resetting the log. It contains no
    /// plaintext, but it can undercount because hashes may collide and it
    /// cannot report values missed by the detectors.
    pub fn summary(&self) -> BTreeMap<String, usize> {
        self.seen
            .iter()
            .map(|(rule, values)| ((*rule).to_string(), values.len()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> (String, Vec<&'static str>) {
        let mut found = Redactions::default();
        let out = redact(text, &mut found);
        assert_eq!(
            out.matches('\n').count(),
            text.matches('\n').count(),
            "redaction changed the line count"
        );
        (out, found.rules())
    }

    #[test]
    fn provider_tokens_become_typed_placeholders() {
        let cases = [
            ("key = AKIAIOSFODNN7EXAMPLE;", "aws-access-key"),
            (
                "t = 'ghp_abcdefghijklmnopqrstuvwxyzABCDEFGHIJ'",
                "github-token",
            ),
            (
                "t = github_pat_11ABCDEFG0123456789_abcdefghijklmnop",
                "github-token",
            ),
            ("GITLAB glpat-abcdefghijklmnopqrst", "gitlab-token"),
            ("slack xoxb-1234567890-abcdefghij", "slack-token"),
            (
                "url https://hooks.slack.com/services/T000/B000/XXXXXXXX",
                "slack-webhook",
            ),
            ("stripe.key(sk_live_abcdefghijklmnop1234)", "stripe-key"),
            (
                "maps AIzaSyA1234567890abcdefghijklmnopqrstuv",
                "google-api-key",
            ),
            ("x sk-ant-api03-abcdefghijklmnopqrstuvwxyz", "anthropic-key"),
            (
                "x sk-proj-abcdefghijklmnopqrstuvwxyz0123456789",
                "openai-key",
            ),
            (
                "Bearer eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.dozjgNryP4J3jVmNHl0w5N_XgL0n3I9PlFUP0THsR8U",
                "jwt",
            ),
        ];
        for (text, rule) in cases {
            let (out, rules) = run(text);
            assert_eq!(rules, vec![rule], "{text}");
            assert!(out.contains(&placeholder(rule)), "{text} -> {out}");
        }
    }

    #[test]
    fn keeps_context_and_replaces_only_the_secret() {
        let (out, _) = run(r#"const password = "hunter2hunter2";"#);
        assert_eq!(out, r#"const password = "<redacted:generic-secret>";"#);

        let (out, _) = run(r#"{"apiKey": "Zq81fKd02nMx", "name": "x"}"#);
        assert_eq!(
            out,
            r#"{"apiKey": "<redacted:generic-secret>", "name": "x"}"#
        );

        let (out, _) = run("postgres://u:p@ss@db/x");
        assert_eq!(out, "postgres://u:<redacted:url-credentials>@db/x");

        // A plain-word URL password is still a password; a template is not.
        let (out, _) = run("postgres://admin:hunter@db/app");
        assert_eq!(out, "postgres://admin:<redacted:url-credentials>@db/app");
        let (out, rules) = run("postgres://admin:${DB_PASS}@db/app");
        assert!(rules.is_empty(), "{out}");

        let (out, _) = run("https://u:pw@host/a@b?c=d@e");
        assert_eq!(out, "https://u:<redacted:url-credentials>@host/a@b?c=d@e");

        let (out, _) = run("postgres://admin:s3cretPass@db:5432/app");
        assert_eq!(
            out,
            "postgres://admin:<redacted:url-credentials>@db:5432/app"
        );

        let (out, _) = run("{ password: 's3cr3t!' }");
        assert_eq!(out, "{ password: '<redacted:generic-secret>' }");

        let (out, _) = run("public testingPassword = 'IamUsedForTesting'");
        assert_eq!(out, "public testingPassword = '<redacted:generic-secret>'");

        let (out, _) = run("+export DB_PASSWORD=hunter22x\n");
        assert_eq!(out, "+export DB_PASSWORD=<redacted:generic-secret>\n");
    }

    #[test]
    fn benign_values_are_left_alone() {
        for text in [
            r#"const TOKEN_TYPE = "access_token";"#,
            r#"const API_KEY_HEADER = "X-Api-Key";"#,
            r#"password = process.env.DB_PASSWORD"#,
            r#"secret: "${SECRET_FROM_ENV}""#,
            r#"password: "********""#,
            r#"token = "short""#,
            r#""Invalid email or password.": "メールアドレスかパスワードが正しくありません","#,
            r#""LABEL_PASSWORD": "Password","#,
            r#""ngx-window-token": "^7.0.0","#,
            r#""LABEL_PASSWORD": "Contraseña","#,
            r#"const BeeTokenAddress = '0x36435796Ca9be2bf150CE0dECc2D8Fab5C4d6E13'"#,
            r#"const possible = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789'"#,
            "let id = \"3f2504e0-4f89-11d3-9a0c-0305e82c3301\";",
            "const sha = \"da39a3ee5e6b4b0d3255bfef95601890afd80709\";",
            "import x from \"./components/some/deeply/nested/Module.ts\";",
            "url = \"https://example.com/path\"",
        ] {
            let (out, rules) = run(text);
            assert!(rules.is_empty(), "{text} redacted as {rules:?}");
            assert_eq!(out, text);
        }
    }

    #[test]
    fn high_entropy_literals_are_caught() {
        let (out, rules) = run(r#"const k = "q8ZrT2vLm9XwP4sB7nYc1HdF6gJ3kA0eQ5uV";"#);
        assert_eq!(rules, vec!["high-entropy"]);
        assert_eq!(out, r#"const k = "<redacted:high-entropy>";"#);
    }

    #[test]
    fn pem_block_in_a_diff_keeps_markers_and_lines() {
        let patch = "@@ -0,0 +1,5 @@\n+const key = `\n+-----BEGIN RSA PRIVATE KEY-----\n+MIIEpAIBAAKCAQEAu1SU1LfVLPHCozMxH2Mo4lgOEePzNm0tRgeLezV6ffAt0gun\n+-----END RSA PRIVATE KEY-----\n+`;";
        let (out, rules) = run(patch);
        assert_eq!(rules, vec!["private-key"]);
        assert_eq!(
            out,
            "@@ -0,0 +1,5 @@\n+const key = `\n+<redacted:private-key>\n+\n+\n+`;"
        );
        assert!(!out.contains("MIIE"));
    }

    #[test]
    fn single_line_pem_collapses() {
        let text =
            r#"key: "-----BEGIN PRIVATE KEY-----\nMIIBVQIBADANBg\n-----END PRIVATE KEY-----\n","#;
        let (out, rules) = run(text);
        assert_eq!(rules, vec!["private-key"]);
        assert_eq!(out, r#"key: "<redacted:private-key>\n","#);
    }

    #[test]
    fn long_pem_is_redacted_through_its_end_marker() {
        let body: Vec<String> = (0..300).map(|i| format!("+QUJD{i:04}RUZH")).collect();
        let patch = format!(
            "+-----BEGIN PRIVATE KEY-----\n{}\n+-----END PRIVATE KEY-----\n+next();",
            body.join("\n")
        );
        let (out, rules) = run(&patch);
        assert_eq!(rules, vec!["private-key"]);
        assert!(
            !out.contains("QUJD"),
            "key body leaked past the old 200-line cap"
        );
        assert!(out.ends_with("+\n+next();"));
    }

    #[test]
    fn pem_marker_in_parser_code_is_left_alone() {
        let text =
            "if pem.starts_with(\"-----BEGIN RSA PRIVATE KEY-----\") {\n    parse_rsa(pem)\n}";
        let (out, rules) = run(text);
        assert!(rules.is_empty());
        assert_eq!(out, text);
    }

    #[test]
    fn truncated_pem_stops_at_the_first_code_line() {
        let (out, rules) = run("+-----BEGIN PRIVATE KEY-----\n+MIIBVQIBADAN\n+fn next() {}");
        assert_eq!(rules, vec!["private-key"]);
        assert_eq!(out, "+<redacted:private-key>\n+\n+fn next() {}");
    }

    #[test]
    fn pem_as_concatenated_string_literals() {
        let text = "const k = '-----BEGIN RSA PRIVATE KEY-----\\n' +\n  'MIIEpAIBAAKCAQEA\\n' +\n  '-----END RSA PRIVATE KEY-----'";
        let (out, rules) = run(text);
        assert_eq!(rules, vec!["private-key"]);
        assert!(!out.contains("MIIE"), "{out}");
    }

    #[test]
    fn unterminated_pem_is_redacted_to_the_end() {
        let (out, rules) = run("-----BEGIN EC PRIVATE KEY-----\nMHcCAQEEIIr\nabc");
        assert_eq!(rules, vec!["private-key"]);
        assert_eq!(out, "<redacted:private-key>\n\n");
    }

    #[test]
    fn redaction_is_idempotent() {
        let text = "password = \"hunter2hunter2\"\nkey = AKIAIOSFODNN7EXAMPLE";
        let (once, _) = run(text);
        let (twice, rules) = run(&once);
        assert_eq!(once, twice);
        assert!(rules.is_empty());
    }

    #[test]
    fn json_values_are_walked_and_keys_kept() {
        let mut state = serde_json::json!({
            "file": { "path": "src/a.ts", "patch": "+const token = 'ghp_abcdefghijklmnopqrstuvwxyzABCDEFGHIJ';" },
            "neighbors": ["AKIAIOSFODNN7EXAMPLE"],
            "line": 3,
        });
        let mut found = Redactions::default();
        redact_value(&mut state, &mut found);
        assert_eq!(found.rules(), vec!["github-token", "aws-access-key"]);
        assert_eq!(state["file"]["path"], "src/a.ts");
        assert_eq!(
            state["file"]["patch"],
            "+const token = '<redacted:github-token>';"
        );
        assert_eq!(state["neighbors"][0], "<redacted:aws-access-key>");
        assert_eq!(state["line"], 3);
    }

    #[test]
    fn log_counts_distinct_values_per_rule() {
        let mut log = RedactionLog::default();
        for _ in 0..3 {
            let mut found = Redactions::default();
            redact("AKIAIOSFODNN7EXAMPLE AKIAIOSFODNN7EXAMPLF", &mut found);
            log.record(found);
        }
        assert_eq!(
            log.summary(),
            BTreeMap::from([("aws-access-key".to_string(), 2)])
        );
    }
}
