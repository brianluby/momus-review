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

/// The new-file line of the hunk's first added line: where a finding in this
/// hunk points, rather than the hunk's leading context. A deletion-only hunk
/// has no added line and falls back to its start line (still inside the diff,
/// so a review comment can anchor there).
pub fn first_added_line(hunk: &Hunk) -> usize {
    let mut line = hunk.start_line;
    for text in hunk.patch.split('\n').skip(1) {
        match text.as_bytes().first() {
            Some(b'+') => return line,
            Some(b' ') => line += 1,
            // `-` removals and `\ No newline at end of file` take no new line.
            _ => {}
        }
    }
    hunk.start_line
}

/// Maps a new-file line to the matching base (pre-change) line using the
/// patch's hunk headers. A line inside a hunk maps to the same offset in the
/// hunk's old range (clamped to it); a line between hunks is shifted by the
/// net lines added or removed before it.
pub fn base_line(patch: &str, new_line: usize) -> usize {
    // Net old-minus-new line shift after the hunks seen so far.
    let mut offset: i64 = 0;
    for header in patch.lines().filter(|l| l.starts_with("@@ ")) {
        let Some((old_start, old_len, new_start, new_len)) = hunk_ranges(header) else {
            continue;
        };
        // A zero-length new range (pure deletion) names the line *before*
        // the change, which keeps its pre-hunk position.
        let before = if new_len == 0 { new_line <= new_start } else { new_line < new_start };
        if before {
            break;
        }
        if new_line < new_start + new_len {
            if old_len == 0 {
                return old_start.max(1);
            }
            return (old_start + (new_line - new_start)).min(old_start + old_len - 1);
        }
        // A zero-length range names the line *before* the change.
        let old_next = if old_len == 0 { old_start + 1 } else { old_start + old_len };
        let new_next = if new_len == 0 { new_start + 1 } else { new_start + new_len };
        offset = old_next as i64 - new_next as i64;
    }
    (new_line as i64 + offset).max(1) as usize
}

/// Parses `@@ -a[,b] +c[,d] @@` into `(a, b, c, d)`; an omitted count is 1.
fn hunk_ranges(header: &str) -> Option<(usize, usize, usize, usize)> {
    let range = |prefix: char| -> Option<(usize, usize)> {
        let token = header.split(' ').find(|t| t.starts_with(prefix))?;
        let mut parts = token[1..].split(',');
        let start = parts.next()?.parse().ok()?;
        let len = match parts.next() {
            Some(n) => n.parse().ok()?,
            None => 1,
        };
        Some((start, len))
    };
    let (old_start, old_len) = range('-')?;
    let (new_start, new_len) = range('+')?;
    Some((old_start, old_len, new_start, new_len))
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
    fn first_added_line_skips_leading_context_and_removals() {
        // Three context lines, one removal, then the addition at new line 13.
        let patch = "@@ -10,5 +10,5 @@\n a\n b\n c\n-old\n+new\n d";
        let hunk = &parse_hunks(patch)[0];
        assert_eq!(hunk.start_line, 10);
        assert_eq!(first_added_line(hunk), 13);

        // An empty context line is a lone space and still advances the count.
        let blank = &parse_hunks("@@ -1,3 +1,4 @@\n x\n \n+y\n z")[0];
        assert_eq!(first_added_line(blank), 3);

        // All additions: the first line of the hunk.
        let added = &parse_hunks("@@ -0,0 +1,2 @@\n+a\n+b")[0];
        assert_eq!(first_added_line(added), 1);
    }

    #[test]
    fn deletion_only_hunk_falls_back_to_its_start() {
        let hunk = &parse_hunks("@@ -4,4 +4,3 @@\n a\n-b\n c\n d\n\\ No newline at end of file")[0];
        assert_eq!(first_added_line(hunk), 4);
    }

    #[test]
    fn base_line_follows_insertions_deletions_and_edits() {
        // Two lines inserted at the top: new 3 is old 1.
        assert_eq!(base_line("@@ -0,0 +1,2 @@\n+a\n+b", 3), 1);
        assert_eq!(base_line("@@ -0,0 +1,2 @@\n+a\n+b", 1), 1);
        // Old lines 3-4 deleted: new 3 is old 5.
        assert_eq!(base_line("@@ -3,2 +2,0 @@\n-x\n-y", 3), 5);
        // ...and new 2, the line before the deletion, is still old 2.
        assert_eq!(base_line("@@ -3,2 +2,0 @@\n-x\n-y", 2), 2);
        // Old 10-12 replaced by new 10-14: inside maps by offset (clamped),
        // after shifts by -2.
        let edit = "@@ -10,3 +10,5 @@\n-a\n-b\n-c\n+1\n+2\n+3\n+4\n+5";
        assert_eq!(base_line(edit, 11), 11);
        assert_eq!(base_line(edit, 14), 12);
        assert_eq!(base_line(edit, 20), 18);
        assert_eq!(base_line(edit, 5), 5);
        // Omitted counts are 1; later hunks accumulate.
        let two = "@@ -1 +1,3 @@\n-a\n+a\n+b\n+c\n@@ -8,2 +10,1 @@\n-x\n-y\n+z";
        assert_eq!(base_line(two, 6), 4);
        assert_eq!(base_line(two, 12), 11);
    }

    #[test]
    fn new_file_patch_is_all_additions_and_chunked() {
        let source = "a\nb\nc";
        let patch = patch_for_new_file(source);
        assert!(patch.starts_with("@@ -0,0 +1,3 @@"));
        assert!(patch.contains("\n+a\n+b\n+c"));
    }
}