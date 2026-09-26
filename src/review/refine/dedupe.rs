//! Dedupe/cluster (#10): fold adjacent findings that share a root cause.
//!
//! Findings are visited in severity order. Each is compared with the already
//! kept findings in the same file within `CLUSTER_LINE_WINDOW` lines through
//! one `choice`: which candidate shares its root cause, or `distinct`. A match
//! at `MIN_CLUSTER_CONFIDENCE` folds it into that candidate's `related` list.
//! Files are independent, so they are processed concurrently.

use std::collections::HashMap;

use futures::{StreamExt, stream};
use serde_json::{Map, Value, json};

use crate::domain::policy::{CLUSTER_LINE_WINDOW, MAX_CLUSTER_CANDIDATES, MIN_CLUSTER_CONFIDENCE};
use crate::domain::report::{Finding, RelatedFinding};
use crate::review::refine::{Refiner, summary};
use crate::review::typesafe::choice;

const DISTINCT: &str = "distinct";

/// Returns the kept findings and how many were folded into another.
pub async fn dedupe(r: &Refiner<'_>, findings: Vec<Finding>) -> (Vec<Finding>, usize) {
    let mut groups: Vec<Vec<Finding>> = Vec::new();
    let mut by_file: HashMap<String, usize> = HashMap::new();
    for finding in findings {
        match by_file.get(&finding.file) {
            Some(&i) => groups[i].push(finding),
            None => {
                by_file.insert(finding.file.clone(), groups.len());
                groups.push(vec![finding]);
            }
        }
    }

    let results: Vec<(Vec<Finding>, usize)> = stream::iter(groups)
        .map(|group| dedupe_file(r, group))
        .buffered(r.concurrency)
        .collect()
        .await;

    let mut kept = Vec::new();
    let mut folded = 0;
    for (group, n) in results {
        kept.extend(group);
        folded += n;
    }
    (kept, folded)
}

async fn dedupe_file(r: &Refiner<'_>, group: Vec<Finding>) -> (Vec<Finding>, usize) {
    let mut reps: Vec<Finding> = Vec::new();
    let mut folded = 0;
    for finding in group {
        let candidates = candidates(&reps, &finding);
        if candidates.is_empty() {
            reps.push(finding);
            continue;
        }
        match same_root_cause(r, &finding, &reps, &candidates).await {
            Ok(Some((index, confidence))) => {
                fold(&mut reps[index], finding, confidence);
                folded += 1;
            }
            Ok(None) => reps.push(finding),
            Err(err) => {
                (r.log)(&format!("  dedupe {}:{} failed (kept): {err:#}", finding.file, finding.line));
                reps.push(finding);
            }
        }
    }
    (reps, folded)
}

/// Indices into `reps` of the kept findings near `finding`, closest first.
fn candidates(reps: &[Finding], finding: &Finding) -> Vec<usize> {
    let mut near: Vec<(usize, usize)> = reps
        .iter()
        .enumerate()
        .filter(|(_, rep)| rep.file == finding.file)
        .map(|(i, rep)| (rep.line.abs_diff(finding.line), i))
        .filter(|(distance, _)| *distance <= CLUSTER_LINE_WINDOW)
        .collect();
    near.sort();
    near.into_iter().take(MAX_CLUSTER_CANDIDATES).map(|(_, i)| i).collect()
}

/// Asks which candidate shares `finding`'s root cause. `Some((rep index,
/// confidence))` on a confident match, `None` when distinct.
async fn same_root_cause(
    r: &Refiner<'_>,
    finding: &Finding,
    reps: &[Finding],
    candidates: &[usize],
) -> anyhow::Result<Option<(usize, f64)>> {
    let mut candidate_state = Map::new();
    let mut criteria = Map::new();
    for (n, &index) in candidates.iter().enumerate() {
        let id = format!("c{n}");
        candidate_state.insert(id.clone(), summary(&reps[index]));
        criteria.insert(
            id.clone(),
            Value::String(format!("Same root cause as candidates.{id}: fixing it would also fix newFinding")),
        );
    }
    criteria.insert(
        DISTINCT.to_string(),
        Value::String("newFinding has a different root cause from every candidate".to_string()),
    );

    let response = r
        .client
        .system_one(
            json!({ "newFinding": summary(finding), "candidates": candidate_state }),
            json!({
                "sameRootCause": choice(
                    json!({
                        "question": "Does newFinding share a root cause with one of the candidates?",
                        "compare": ["newFinding", "candidates"],
                        "caution": "Findings in the same file or dimension can still have different root causes; require the same underlying defect",
                    }),
                    Value::Object(criteria),
                ),
            }),
        )
        .await?;
    let (label, confidence) = response.choice("sameRootCause")?;
    Ok(resolve(&label, confidence, candidates))
}

/// Maps a `sameRootCause` answer to a rep index when it is a confident match.
fn resolve(label: &str, confidence: f64, candidates: &[usize]) -> Option<(usize, f64)> {
    if confidence < MIN_CLUSTER_CONFIDENCE {
        return None;
    }
    let n: usize = label.strip_prefix('c')?.parse().ok()?;
    candidates.get(n).map(|&index| (index, confidence))
}

fn fold(rep: &mut Finding, finding: Finding, confidence: f64) {
    rep.related.push(RelatedFinding {
        file: finding.file,
        line: finding.line,
        dimension: finding.dimension,
        mechanism: finding.mechanism,
        confidence,
    });
    rep.related.extend(finding.related);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::policy::Dimension;

    fn at(file: &str, line: usize) -> Finding {
        Finding { file: file.into(), line, ..Default::default() }
    }

    #[test]
    fn candidates_are_same_file_within_window_closest_first() {
        let reps = vec![at("a.rs", 10), at("b.rs", 12), at("a.rs", 200), at("a.rs", 30)];
        assert_eq!(candidates(&reps, &at("a.rs", 25)), vec![3, 0]);
        assert!(candidates(&reps, &at("c.rs", 10)).is_empty());
    }

    #[test]
    fn resolve_needs_a_confident_known_candidate() {
        let cands = [4, 7];
        assert_eq!(resolve("c1", 0.9, &cands), Some((7, 0.9)));
        assert_eq!(resolve("c1", MIN_CLUSTER_CONFIDENCE - 0.01, &cands), None);
        assert_eq!(resolve(DISTINCT, 0.99, &cands), None);
        assert_eq!(resolve("c9", 0.99, &cands), None);
    }

    #[test]
    fn fold_carries_nested_related() {
        let mut rep = at("a.rs", 10);
        let mut other = Finding { dimension: Dimension::Security, mechanism: "xss".into(), ..at("a.rs", 12) };
        other.related.push(RelatedFinding { file: "a.rs".into(), line: 14, ..Default::default() });
        fold(&mut rep, other, 0.8);
        assert_eq!(rep.related.len(), 2);
        assert_eq!(rep.related[0].line, 12);
        assert_eq!(rep.related[0].mechanism, "xss");
        assert_eq!(rep.related[1].line, 14);
    }
}
