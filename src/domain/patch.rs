//! Unified-diff helpers shared by the Git adapter and the review workflow.
//!
//! These helpers consume the plain, per-file unified diffs produced by the Git
//! adapter. They are evidence and context helpers, not patch validators or a
//! patch application engine. Callers supplying their own text must keep hunk
//! ranges ordered, consistent with the body, and within ordinary file bounds;
//! combined diffs and concatenated multi-file patches are outside this contract.
//! Line numbers refer to the new file unless a function explicitly says base.

use crate::domain::report::Hunk;

const UNTRACKED_CHUNK_LINES: usize = 80;

/// Extracts hunks from a per-file unified diff in encounter order.
///
/// File headers before the first `@@ ` line are discarded. Each recognized
/// header opens a hunk, whose `start_line` comes from the header's `+` range and
/// whose ID is `hunk_1`, `hunk_2`, and so on, local to this call. An unparseable
/// new start defaults to 1; text without a recognized header returns no hunks.
/// Neither outcome establishes that the input is a valid or empty change.
/// Header counts and body lines are not validated.
///
/// The start is usually one-based, but a zero-length range can name line 0
/// before the beginning of the file. Use [`first_added_line`] to skip leading
/// context when locating an addition, and check commentability separately
/// before publishing an inline comment.
///
/// ```
/// use momus_review::domain::patch::{base_line, first_added_line, parse_hunks};
///
/// let hunks = parse_hunks("--- a.rs\n+++ a.rs\n@@ -10,3 +10,3 @@\n before\n-old\n+new\n after");
/// assert_eq!(hunks.len(), 1);
/// assert_eq!(hunks[0].id, "hunk_1");
/// assert_eq!(hunks[0].start_line, 10);
/// assert_eq!(first_added_line(&hunks[0]), 11);
///
/// let deletion = parse_hunks("@@ -3,2 +2,0 @@\n-x\n-y");
/// assert_eq!(first_added_line(&deletion[0]), 2); // Fallback, not an added line.
/// assert_eq!(base_line(&deletion[0].patch, 3), 5);
/// assert!(parse_hunks("no unified hunk header").is_empty());
/// ```
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

/// Returns the new-file line of the first addition, skipping leading context.
///
/// Context lines advance the new-file position; removals and no-newline markers
/// do not. The first line of `hunk.patch` is assumed to be its header, and
/// `hunk.start_line` must agree with that header. No consistency check is made.
///
/// If there is no addition, the result is `hunk.start_line`. For a pure deletion
/// that is the position before the deleted range, which may be 0 or outside the
/// lines available for a RIGHT-side inline comment. The caller must validate
/// that fallback against the destination's commenting rules.
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

/// Splits `hunk` into consecutive sub-hunks, starting a new piece at each
/// new-file line in `cuts`, so a large hunk (a whole new file, a rewritten
/// function) offers smaller evidence candidates and a finding anchors near
/// its code instead of at the hunk's first line. For a valid input hunk, each
/// piece has an `@@` header with counts recomputed from its body. Removed lines
/// stay with the lines that follow them (a replacement stays whole); cuts that
/// fall outside the hunk or would
/// leave a piece with no new-side line are ignored. Cuts are absolute new-file
/// line numbers, not offsets into `hunk.patch`; their order and duplicates do
/// not create additional cuts.
///
/// If the header cannot be parsed, or no cut produces multiple pieces, a clone
/// of the original hunk is returned, including its ID. Actual split pieces have
/// empty IDs: the caller must assign IDs before presenting selectable evidence.
/// This function does not validate the original header counts or patch body.
///
/// ```
/// use momus_review::domain::patch::{first_added_line, parse_hunks, split_hunk};
///
/// let original = parse_hunks("@@ -10,3 +10,3 @@\n before\n-old\n+new\n after");
/// let mut pieces = split_hunk(&original[0], &[11]);
/// assert_eq!(pieces.len(), 2);
/// assert_eq!(pieces[0].patch, "@@ -10,1 +10,1 @@\n before");
/// assert_eq!(pieces[1].patch, "@@ -11,2 +11,2 @@\n-old\n+new\n after");
/// assert_eq!(first_added_line(&pieces[1]), 11);
/// assert!(pieces.iter().all(|piece| piece.id.is_empty()));
/// for (index, piece) in pieces.iter_mut().enumerate() {
///     piece.id = format!("candidate_{}", index + 1);
/// }
/// assert_eq!(pieces[1].id, "candidate_2");
/// ```
pub fn split_hunk(hunk: &Hunk, cuts: &[usize]) -> Vec<Hunk> {
    let mut body = hunk.patch.split('\n');
    let Some((old_start, old_len, new_start, new_len)) = body.next().and_then(hunk_ranges) else {
        return vec![hunk.clone()];
    };
    // The next old/new line numbers; a zero-length range names the line before.
    let mut old_next = if old_len == 0 { old_start + 1 } else { old_start };
    let mut new_next = if new_len == 0 { new_start + 1 } else { new_start };

    struct Piece<'a> {
        lines: Vec<&'a str>,
        old_first: usize,
        new_first: usize,
        old_count: usize,
        new_count: usize,
    }
    impl Piece<'_> {
        fn hunk(&self) -> Hunk {
            let old_start = if self.old_count == 0 { self.old_first - 1 } else { self.old_first };
            let new_start = if self.new_count == 0 { self.new_first - 1 } else { self.new_first };
            let mut patch =
                format!("@@ -{old_start},{} +{new_start},{} @@", self.old_count, self.new_count);
            for line in &self.lines {
                patch.push('\n');
                patch.push_str(line);
            }
            Hunk { id: String::new(), start_line: new_start, patch }
        }
    }
    let removals = |lines: &[&str]| lines.iter().filter(|l| l.starts_with('-')).count();
    let new_piece = |old_first, new_first| Piece {
        lines: Vec::new(),
        old_first,
        new_first,
        old_count: 0,
        new_count: 0,
    };

    let mut pieces: Vec<Piece> = Vec::new();
    let mut current = new_piece(old_next, new_next);
    // Removals (and their no-newline markers) wait for the next kept line.
    let mut pending: Vec<&str> = Vec::new();
    let mut pending_old_first = old_next;
    let mut last_was_removal = false;

    for line in body {
        match line.as_bytes().first() {
            Some(b'-') => {
                if pending.is_empty() {
                    pending_old_first = old_next;
                }
                pending.push(line);
                old_next += 1;
                last_was_removal = true;
            }
            Some(b'+') | Some(b' ') => {
                if cuts.contains(&new_next) && current.new_count > 0 {
                    let first_old = if pending.is_empty() { old_next } else { pending_old_first };
                    pieces.push(std::mem::replace(&mut current, new_piece(first_old, new_next)));
                }
                current.old_count += removals(&pending);
                current.lines.append(&mut pending);
                current.lines.push(line);
                current.new_count += 1;
                if line.starts_with(' ') {
                    current.old_count += 1;
                    old_next += 1;
                }
                new_next += 1;
                last_was_removal = false;
            }
            // `\ No newline at end of file` belongs to the line before it.
            Some(b'\\') if last_was_removal => pending.push(line),
            Some(b'\\') => current.lines.push(line),
            _ => {}
        }
    }
    current.old_count += removals(&pending);
    current.lines.append(&mut pending);
    pieces.push(current);

    if pieces.len() == 1 {
        return vec![hunk.clone()];
    }
    pieces.iter().map(Piece::hunk).collect()
}

/// Chooses a nearby base (pre-change) line for a new-file context window.
///
/// A line inside a hunk maps to the same offset in the
/// hunk's old range (clamped to it); a line between hunks is shifted by the
/// net lines added or removed before it.
///
/// This is a positional approximation, not an assertion that the lines contain
/// the same text. Added lines have no exact base counterpart; an all-additions
/// range maps to its old start, clamped to at least 1. A zero-length new range
/// names the line before a deletion and leaves that line's position unchanged.
/// Malformed headers are skipped, and an empty patch gives `new_line.max(1)`.
/// Callers must supply a one-based line and ordered, bounded ranges and must
/// clamp the result to actual base content when extracting a window.
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
///
/// Each hunk contains at most 80 newline-separated segments, with new-file
/// starts continuing across hunks. This is a synthetic review representation:
/// there are no file headers or no-newline markers, and an empty source or a
/// trailing newline contributes an empty added segment. Do not use the result
/// to reconstruct the source's exact final-newline state or as an applyable
/// file patch without the required file metadata.
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

    /// Each piece's header counts match its body, and the bodies rejoin to
    /// the original hunk's body.
    fn assert_valid_split(original: &Hunk, pieces: &[Hunk]) {
        let mut rejoined = Vec::new();
        for piece in pieces {
            let mut lines = piece.patch.split('\n');
            let (_, old_len, new_start, new_len) = hunk_ranges(lines.next().unwrap()).unwrap();
            let body: Vec<&str> = lines.collect();
            let old = body.iter().filter(|l| l.starts_with(' ') || l.starts_with('-')).count();
            let new = body.iter().filter(|l| l.starts_with(' ') || l.starts_with('+')).count();
            assert_eq!((old, new), (old_len, new_len), "{}", piece.patch);
            assert_eq!(piece.start_line, new_start);
            rejoined.extend(body);
        }
        let original_body: Vec<&str> = original.patch.split('\n').skip(1).collect();
        assert_eq!(rejoined, original_body);
    }

    #[test]
    fn a_new_file_splits_into_pieces_that_anchor_on_their_own_lines() {
        let body: String = (1..=10).map(|i| format!("\n+line {i}")).collect();
        let hunk = &parse_hunks(&format!("@@ -0,0 +1,10 @@{body}"))[0];
        let pieces = split_hunk(hunk, &[4, 8]);
        let headers: Vec<&str> = pieces.iter().map(|p| p.patch.split('\n').next().unwrap()).collect();
        assert_eq!(headers, ["@@ -0,0 +1,3 @@", "@@ -0,0 +4,4 @@", "@@ -0,0 +8,3 @@"]);
        let anchors: Vec<usize> = pieces.iter().map(first_added_line).collect();
        assert_eq!(anchors, [1, 4, 8]);
        assert_valid_split(hunk, &pieces);
    }

    #[test]
    fn a_replacement_stays_whole_and_old_lines_are_counted() {
        // old 10..15 = a b c d e f; new 10..15 = a b C d e f; cut at the new line 12 (C).
        let hunk = &parse_hunks("@@ -10,6 +10,6 @@\n a\n b\n-c\n+C\n d\n e\n f")[0];
        let pieces = split_hunk(hunk, &[12]);
        assert_eq!(pieces.len(), 2);
        assert_eq!(pieces[0].patch, "@@ -10,2 +10,2 @@\n a\n b");
        assert_eq!(pieces[1].patch, "@@ -12,4 +12,4 @@\n-c\n+C\n d\n e\n f");
        assert_eq!(first_added_line(&pieces[1]), 12);
        assert_valid_split(hunk, &pieces);
    }

    #[test]
    fn cuts_outside_the_hunk_or_at_its_start_change_nothing() {
        let hunk = &parse_hunks("@@ -1,3 +1,3 @@\n a\n-b\n+B\n c")[0];
        for cuts in [vec![], vec![1], vec![50], vec![0, 1, 99]] {
            let pieces = split_hunk(hunk, &cuts);
            assert_eq!(pieces.len(), 1, "cuts {cuts:?}");
            assert_eq!(pieces[0].patch, hunk.patch);
        }
    }

    #[test]
    fn a_no_newline_marker_stays_with_its_line() {
        let hunk = &parse_hunks("@@ -1,2 +1,3 @@\n a\n+b\n-c\n\\ No newline at end of file\n+d")[0];
        let pieces = split_hunk(hunk, &[3]);
        assert_eq!(pieces.len(), 2);
        assert_eq!(pieces[1].patch, "@@ -2,1 +3,1 @@\n-c\n\\ No newline at end of file\n+d");
        assert_valid_split(hunk, &pieces);
    }

    #[test]
    fn a_long_mixed_hunk_splits_validly_at_every_cut() {
        // 40 lines alternating context, replacements, and additions.
        let mut body = String::new();
        for i in 0..40 {
            match i % 4 {
                0 => body.push_str(&format!("\n ctx{i}")),
                1 => body.push_str(&format!("\n-old{i}\n+new{i}")),
                2 => body.push_str(&format!("\n+add{i}")),
                _ => body.push_str(&format!("\n-gone{i}")),
            }
        }
        let old = 10 + 10 + 10; // ctx + old + gone
        let new = 10 + 10 + 10; // ctx + new + add
        let hunk = &parse_hunks(&format!("@@ -100,{old} +200,{new} @@{body}"))[0];
        let pieces = split_hunk(hunk, &[205, 211, 219, 226]);
        assert_eq!(pieces.len(), 5);
        assert_valid_split(hunk, &pieces);
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
