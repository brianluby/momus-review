//! Compiled exclusion globs: filter discovered paths before they are read or
//! screened. Patterns follow gitignore semantics against repo-relative,
//! forward-slash paths:
//!
//! * `*` and `?` never cross `/`; `**` matches across directories.
//! * A pattern with no `/` (other than a trailing one) matches at any depth:
//!   `node_modules` excludes `a/node_modules/x.js`.
//! * A pattern containing `/` is anchored at the repo root; a leading `/` is
//!   accepted and stripped.
//! * A pattern also excludes everything beneath a matching directory, and a
//!   trailing `/` (`vendor/`) matches directories only.
//! * Negation (`!pattern`) is not supported and is rejected.

use anyhow::bail;
use globset::{GlobBuilder, GlobSet, GlobSetBuilder};

#[derive(Clone, Default)]
pub struct Exclude {
    set: GlobSet,
}

impl Exclude {
    /// Compiles the patterns into a matcher. `frontend/src/assets/**` excludes
    /// that whole subtree; `*.min.js` excludes by name at any depth.
    pub fn new(patterns: &[String]) -> anyhow::Result<Self> {
        let mut builder = GlobSetBuilder::new();
        for pattern in patterns {
            let trimmed = pattern.trim();
            if trimmed.is_empty() {
                continue;
            }
            if trimmed.starts_with('!') {
                bail!("negated exclude patterns are not supported: {trimmed}");
            }
            for glob in expand(trimmed) {
                builder.add(GlobBuilder::new(&glob).literal_separator(true).build()?);
            }
        }
        Ok(Exclude { set: builder.build()? })
    }

    /// Whether `path` (a repo-relative, forward-slash path) matches any pattern.
    pub fn is_match(&self, path: &str) -> bool {
        self.set.is_match(path)
    }

    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }
}

/// Expands one gitignore-style pattern into the equivalent file-path globs.
fn expand(pattern: &str) -> Vec<String> {
    let dir_only = pattern.ends_with('/');
    let body = pattern.trim_end_matches('/');
    let anchored = body.contains('/');
    let body = body.trim_start_matches('/');
    let base = if anchored || body.starts_with("**/") {
        body.to_string()
    } else {
        format!("**/{body}")
    };

    // Discovered paths are files, so a directory pattern matches via its
    // contents; a non-directory pattern may also name a file directly.
    let mut globs = vec![format!("{base}/**")];
    if !dir_only {
        globs.push(base);
    }
    globs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exclude(patterns: &[&str]) -> Exclude {
        Exclude::new(&patterns.iter().map(|p| p.to_string()).collect::<Vec<_>>()).unwrap()
    }

    #[test]
    fn trailing_slash_excludes_directory_contents() {
        let e = exclude(&["vendor/"]);
        assert!(e.is_match("vendor/lib.js"));
        assert!(e.is_match("a/vendor/deep/lib.js"));
        assert!(!e.is_match("vendor.js"));
    }

    #[test]
    fn bare_name_matches_at_any_depth() {
        let e = exclude(&["node_modules", "*.min.js"]);
        assert!(e.is_match("node_modules/x.js"));
        assert!(e.is_match("web/node_modules/x/y.js"));
        assert!(e.is_match("dist/app.min.js"));
        assert!(!e.is_match("src/app.js"));
    }

    #[test]
    fn slash_patterns_are_anchored_and_star_stays_in_segment() {
        let e = exclude(&["src/*.rs"]);
        assert!(e.is_match("src/lib.rs"));
        assert!(!e.is_match("src/deep/nested.rs"));
        assert!(!e.is_match("other/src/lib.rs"));

        let rooted = exclude(&["/gen"]);
        assert!(rooted.is_match("gen/a.ts"));
        assert!(!rooted.is_match("src/gen/a.ts"));
    }

    #[test]
    fn double_star_subtree_and_exact_file() {
        let e = exclude(&["frontend/src/assets/**", "data/static/contractABIs.ts"]);
        assert!(e.is_match("frontend/src/assets/i18n/en.ts"));
        assert!(e.is_match("data/static/contractABIs.ts"));
        assert!(!e.is_match("frontend/src/app.ts"));
    }

    #[test]
    fn negation_is_rejected() {
        assert!(Exclude::new(&["!keep.rs".to_string()]).is_err());
    }
}
