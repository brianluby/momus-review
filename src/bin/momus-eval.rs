//! Golden-set evaluation: measure category coverage and curated
//! corroboration of a momus report against a ground-truth mapping.
//!
//! Usage: `momus-eval <report.json> <ground-truth.json>`
//!
//! Ground-truth JSON: `{ "categories": { "<category>": <challenge-count> },
//! "known_vulnerable": [{ "file": "<prefix>", "categories": ["<category>"] }] }`.
//! Coverage maps a finding's security mechanism to the OWASP categories it
//! can indicate (a deliberately loose, security-scoped mapping).
//! Corroboration matches findings against the curated known-vulnerable file
//! list (a lower bound, not a precision estimate — uncurated routes are not
//! counted as false positives).

use std::collections::BTreeMap;

use anyhow::Result;
use serde::Deserialize;

use momus_review::domain::policy::Dimension;
use momus_review::domain::report::ReviewReport;

#[derive(Deserialize)]
struct GroundTruth {
    categories: BTreeMap<String, usize>,
    #[serde(default)]
    known_vulnerable: Vec<KnownVulnerable>,
}

#[derive(Deserialize)]
struct KnownVulnerable {
    file: String,
    categories: Vec<String>,
}

/// A security mechanism maps to the OWASP categories it can indicate.
fn mechanism_categories(m: &str) -> &'static [&'static str] {
    match m {
        "authorization" => &[
            "Broken Access Control",
            "Broken Authentication",
            "Unvalidated Redirects",
        ],
        "injection" => &["Injection", "XSS", "XXE", "Insecure Deserialization"],
        "exposure" => &["Sensitive Data Exposure", "Observability Failures"],
        "unsafeDefault" => &["Security Misconfiguration"],
        "other" => &["Miscellaneous"],
        _ => &[],
    }
}

/// A curated `known_vulnerable.file` prefix matches a finding path only when
/// it is the whole path or a `/` path-component boundary follows, so a
/// sibling like `routes/login.ts.bak` does not match `routes/login.ts`.
fn matches_curated_file(path: &str, prefix: &str) -> bool {
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with('/'))
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("usage: momus-eval <report.json> <ground-truth.json>");
        std::process::exit(2);
    }

    let report: ReviewReport = serde_json::from_str(&std::fs::read_to_string(&args[1])?)?;
    let gt: GroundTruth = serde_json::from_str(&std::fs::read_to_string(&args[2])?)?;

    let security: Vec<_> = report
        .findings
        .iter()
        .filter(|f| f.dimension == Dimension::Security)
        .collect();

    // 1. Category coverage.
    let mut touched: BTreeMap<String, usize> = BTreeMap::new();
    for f in &security {
        for c in mechanism_categories(&f.mechanism) {
            *touched.entry((*c).to_string()).or_insert(0) += 1;
        }
    }

    println!("== category coverage ==");
    let covered = gt
        .categories
        .keys()
        .filter(|c| touched.contains_key(*c))
        .count();
    println!("categories touched: {covered}/{}", gt.categories.len());
    for (cat, count) in &gt.categories {
        let ours = touched.get(cat).copied().unwrap_or(0);
        println!("  {} {cat}: {ours} findings vs {count} challenges", if ours > 0 { '✓' } else { '✗' });
    }

    // 2. Curated corroboration rate (only meaningful once known_vulnerable is
    // curated; a lower bound, not precision).
    if !gt.known_vulnerable.is_empty() {
        println!();
        println!("== curated corroboration rate ==");
        let mut matched = 0;
        let mut mismatched: Vec<String> = Vec::new();
        for f in &security {
            if let Some(kv) = gt
                .known_vulnerable
                .iter()
                .find(|kv| matches_curated_file(&f.file, &kv.file))
            {
                let cat_ok = kv
                    .categories
                    .iter()
                    .any(|c| mechanism_categories(&f.mechanism).contains(&c.as_str()));
                if cat_ok {
                    matched += 1;
                } else {
                    mismatched.push(format!(
                        "{} ({} maps to {:?}, expected {:?})",
                        f.file,
                        f.mechanism,
                        mechanism_categories(&f.mechanism),
                        kv.categories
                    ));
                }
            }
        }
        let found_files = gt
            .known_vulnerable
            .iter()
            .filter(|kv| security.iter().any(|f| matches_curated_file(&f.file, &kv.file)))
            .count();
        println!(
            "known-vulnerable files hit by a matching finding: {found_files}/{}",
            gt.known_vulnerable.len()
        );
        println!(
            "security findings on known-vulnerable files (category-matched): {matched}/{}",
            security.len()
        );
        for m in mismatched {
            println!("  ! {m}");
        }
    }

    Ok(())
}