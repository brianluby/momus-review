//! Unified-diff helpers shared by the Git adapter and the review workflow.
//! Mirrors `domain/patch.ts`.

use crate::domain::report::Hunk;

const UNTRACKED_CHUNK_LINES: usize = 80;

/// Splits a unified diff into hunks, tracking the new-file start line of each.
pub fn parse_hunks(patch: &str) -> Vec<Hunk> {
    let mut hunks: Vec<Hunk> = Vec::new();
    let mut current: Option<Vec<&str>> = None;
    let mut start_line = 1usize;

    fn flush(hunks: &mut Vec<Hunk>, current: &mut Option<Vec<&str>>, start_line: usize) {
        if let Some(lines) = current.take() {
            hunks.push(Hunk {
                id: format!("hunk_{}", hunks.len() + 1),
                start_line,
                patch: lines.join("\n"),
            });
        }
    }

    for line in patch.split('\n') {
        if line.starts_with("@@ ") {
            // Flush the previous hunk using the start line assigned when it
            // was opened, then open the next hunk with the new start line.
            flush(&mut hunks, &mut current, start_line);
            start_line = line
                .split(' ')
                .find(|t| t.starts_with('+'))
                .and_then(|t| t[1..].split(',').next())
                .and_then(|n| n.parse().ok())
                .unwrap_or(1);
            current = Some(vec![line]);
        } else if let Some(lines) = current.as_mut() {
            lines.push(line);
        }
    }
    flush(&mut hunks, &mut current, start_line);
    hunks
}

/// Renders a brand-new file as an all-additions diff, chunked so hunk
/// selection still points at a specific region of the file.
pub fn patch_for_new_file(source: &str) -> String {
    let lines: Vec<&str> = source.split('\n').collect();
    let chunk_count = lines.len().div_ceil(UNTRACKED_CHUNK_LINES);

    (0..chunk_count)
        .map(|index| {
            let start = index * UNTRACKED_CHUNK_LINES;
            let chunk = &lines[start..lines.len().min(start + UNTRACKED_CHUNK_LINES)];
            let mut out = format!("@@ -0,0 +{},{} @@", start + 1, chunk.len());
            for line in chunk {
                out.push_str(&format!("\n+{line}"));
            }
            out
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hunks_with_start_lines() {
        let patch = "@@ -1,0 +1,2 @@\n+foo\n+bar\n@@ -5,0 +7,1 @@\n+baz";
        let hunks = parse_hunks(patch);
        assert_eq!(hunks.len(), 2);
        assert_eq!(hunks[0].id, "hunk_1");
        assert_eq!(hunks[0].start_line, 1);
        assert_eq!(hunks[1].id, "hunk_2");
        assert_eq!(hunks[1].start_line, 7);
    }

    #[test]
    fn new_file_patch_is_all_additions_and_chunked() {
        let source = "a\nb\nc";
        let patch = patch_for_new_file(source);
        assert!(patch.starts_with("@@ -0,0 +1,3 @@"));
        assert!(patch.contains("\n+a\n+b\n+c"));
    }
}