//! Pairwise severity ranking (#12): "which should be fixed first?" for every
//! pair among the top-K findings, then a Bradley-Terry fit for a stable order.
//!
//! Absolute severity scores bunch together (many findings land near 2.5); a
//! relative judgment separates them. Each answer's confidence is a soft win.
//! Question order alternates within pairs to cancel position bias.

use futures::{StreamExt, stream};
use serde_json::{Map, Value, json};

use crate::domain::policy::{PAIRWISE_BATCH, PAIRWISE_TOP_K};
use crate::domain::report::Finding;
use crate::review::refine::{Refiner, summary};
use crate::review::typesafe::choice;

/// Re-orders the top-K of `findings` (sorted by severity) by pairwise
/// strength and sets `rank`. Leaves the order untouched on any failure.
pub async fn rank(r: &Refiner<'_>, findings: &mut [Finding]) {
    let k = PAIRWISE_TOP_K.min(findings.len());
    if k < 2 {
        return;
    }
    let pairs: Vec<(usize, usize)> = (0..k).flat_map(|i| (i + 1..k).map(move |j| (i, j))).collect();
    let mut state_findings = Map::new();
    for (i, finding) in findings[..k].iter().enumerate() {
        state_findings.insert(format!("f{i}"), summary(finding));
    }
    let state = json!({ "findings": state_findings });

    let batches: Vec<Vec<(usize, usize)>> = pairs.chunks(PAIRWISE_BATCH).map(<[_]>::to_vec).collect();
    let answers: Vec<anyhow::Result<Vec<(usize, usize, f64)>>> = stream::iter(batches)
        .map(|batch| {
            let state = state.clone();
            async move { compare(r, state, &batch).await }
        })
        .buffered(r.concurrency)
        .collect()
        .await;

    let mut outcomes = Vec::new();
    for answer in answers {
        match answer {
            Ok(batch) => outcomes.extend(batch),
            Err(err) => {
                (r.log)(&format!("  pairwise ranking failed (severity order kept): {err:#}"));
                return;
            }
        }
    }

    let strength = bradley_terry(k, &outcomes);
    let mut order: Vec<usize> = (0..k).collect();
    order.sort_by(|&a, &b| strength[b].total_cmp(&strength[a]).then(a.cmp(&b)));
    let ranked: Vec<Finding> = order.iter().map(|&i| findings[i].clone()).collect();
    for (position, mut finding) in ranked.into_iter().enumerate() {
        finding.rank = Some(position + 1);
        findings[position] = finding;
    }
}

/// Asks one batch of pair questions; returns `(i, j, P(i before j))`.
async fn compare(
    r: &Refiner<'_>,
    state: Value,
    batch: &[(usize, usize)],
) -> anyhow::Result<Vec<(usize, usize, f64)>> {
    let mut questions = Map::new();
    for &(i, j) in batch {
        let (first, second) = if (i + j) % 2 == 0 { (i, j) } else { (j, i) };
        let mut criteria = Map::new();
        for n in [first, second] {
            criteria.insert(format!("f{n}"), Value::String(format!("Fix findings.f{n} first")));
        }
        questions.insert(
            format!("p{i}_{j}"),
            choice(
                json!({
                    "question": format!("Which should be fixed first: findings.f{first} or findings.f{second}?"),
                    "focus": "Likely production impact, exploitability or blast radius, and how directly the evidence shows the problem",
                }),
                Value::Object(criteria),
            ),
        );
    }

    let response = r.client.system_one(state, Value::Object(questions)).await?;
    batch
        .iter()
        .map(|&(i, j)| {
            let (label, confidence) = response.choice(&format!("p{i}_{j}"))?;
            let p_i = if label == format!("f{i}") { confidence } else { 1.0 - confidence };
            Ok((i, j, p_i))
        })
        .collect()
}

/// Bradley-Terry strengths for `k` items from soft outcomes `(i, j, P(i beats
/// j))`, fit by minorization-maximization and normalized to mean 1. A light
/// symmetric prior keeps strengths finite when an item never wins.
pub fn bradley_terry(k: usize, outcomes: &[(usize, usize, f64)]) -> Vec<f64> {
    const PRIOR: f64 = 0.1;
    let mut wins = vec![vec![0.0; k]; k];
    for &(i, j, p) in outcomes {
        wins[i][j] += p;
        wins[j][i] += 1.0 - p;
    }
    for (i, row) in wins.iter_mut().enumerate() {
        for (j, w) in row.iter_mut().enumerate() {
            if i != j {
                *w += PRIOR;
            }
        }
    }

    let mut strength = vec![1.0; k];
    for _ in 0..200 {
        let mut next: Vec<f64> = (0..k)
            .map(|i| {
                let total_wins: f64 = (0..k).filter(|&j| j != i).map(|j| wins[i][j]).sum();
                let denom: f64 = (0..k)
                    .filter(|&j| j != i)
                    .map(|j| (wins[i][j] + wins[j][i]) / (strength[i] + strength[j]))
                    .sum();
                total_wins / denom
            })
            .collect();
        let mean = next.iter().sum::<f64>() / k as f64;
        next.iter_mut().for_each(|s| *s /= mean);
        strength = next;
    }
    strength
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn transitive_wins_order_items() {
        // 2 beats everyone, 0 beats 1.
        let outcomes = [(0, 1, 0.9), (0, 2, 0.2), (1, 2, 0.1)];
        let s = bradley_terry(3, &outcomes);
        assert!(s[2] > s[0] && s[0] > s[1], "{s:?}");
        assert!((s.iter().sum::<f64>() - 3.0).abs() < 1e-9);
    }

    #[test]
    fn coin_flips_are_ties() {
        let outcomes = [(0, 1, 0.5), (0, 2, 0.5), (1, 2, 0.5)];
        let s = bradley_terry(3, &outcomes);
        assert!(s.iter().all(|v| (v - 1.0).abs() < 1e-9), "{s:?}");
    }

    #[test]
    fn never_winning_stays_finite() {
        let outcomes = [(0, 1, 1.0), (0, 2, 1.0), (1, 2, 1.0)];
        let s = bradley_terry(3, &outcomes);
        assert!(s.iter().all(|v| v.is_finite() && *v > 0.0), "{s:?}");
        assert!(s[0] > s[1] && s[1] > s[2]);
    }
}
