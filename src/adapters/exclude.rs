//! Compiled exclusion globs: filter discovered paths before they are read or
//! screened. Patterns use gitignore-style globs (forward slashes, `**` matches
//! across directories).

use globset::{Glob, GlobSet, GlobSetBuilder};

#[derive(Clone, Default)]
pub struct Exclude {
    set: GlobSet,
}

impl Exclude {
    /// Compiles the patterns into a matcher. `frontend/src/assets/**` excludes
    /// that whole subtree; `**/*.min.js` excludes by name.
    pub fn new(patterns: &[String]) -> anyhow::Result<Self> {
        let mut builder = GlobSetBuilder::new();
        for pattern in patterns {
            let trimmed = pattern.trim();
            if trimmed.is_empty() {
                continue;
            }
            builder.add(Glob::new(trimmed)?);
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