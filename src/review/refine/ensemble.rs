//! Ensembles on demand (#15): re-screen high-stakes findings under varied
//! focus and route to a human on disagreement.
//!
//! Only blocking (`request_changes`) findings are re-asked, at most
//! `MAX_ENSEMBLE`. One request carries one dimension-level `noul` per
//! `ENSEMBLE_FOCI` perspective, independent of the classified mechanism.
//! A spread of at least `NEEDS_HUMAN_SPREAD`, or a mean below
//! `NEEDS_HUMAN_MEAN`, sets `needs_human`; the finding is kept either way.

use futures::{StreamExt, stream};
use serde_json::{Map, Value, json};

use crate::domain::policy::{ENSEMBLE_FOCI, MAX_ENSEMBLE, NEEDS_HUMAN_MEAN, NEEDS_HUMAN_SPREAD};
use crate::domain::report::{Action, Ensemble, Finding};
use crate::review::refine::{Refiner, finding_state, top_indices};
use crate::review::typesafe::noul;

/// Votes on the top `MAX_ENSEMBLE` blocking findings in place.
pub async fn vote(r: &Refiner<'_>, findings: &mut [Finding]) {
    let targets = top_indices(findings, MAX_ENSEMBLE, |f| f.action == Action::RequestChanges);
    let findings_ref: &[Finding] = findings;
    let results: Vec<(usize, anyhow::Result<Vec<f64>>)> = stream::iter(targets)
        .map(|i| async move { (i, ask(r, &findings_ref[i]).await) })
        .buffered(r.concurrency)
        .collect()
        .await;

    for (i, result) in results {
        match result {
            Ok(votes) => findings[i].ensemble = Some(tally(votes)),
            Err(err) => (r.log)(&format!(
                "  ensemble {}:{} failed (kept): {err:#}",
                findings[i].file, findings[i].line
            )),
        }
    }
}

async fn ask(r: &Refiner<'_>, finding: &Finding) -> anyhow::Result<Vec<f64>> {
    let question = format!(
        "Does selectedEvidence directly support a concrete {} problem: {}",
        finding.dimension.key(),
        finding.dimension.definition()
    );
    let mut questions = Map::new();
    for (key, focus) in ENSEMBLE_FOCI {
        questions.insert(
            key.to_string(),
            noul(
                json!({
                    "question": question,
                    "inspect": ["selectedEvidence", "fileContext"],
                    "focus": focus,
                }),
                json!({
                    "true": { "what": "The code concretely exhibits the problem" },
                    "false": { "what": "The problem is not concretely supported" },
                }),
            ),
        );
    }
    let response = r
        .client
        .system_one(finding_state(r, finding), Value::Object(questions))
        .await?;
    ENSEMBLE_FOCI.iter().map(|(key, _)| response.noul(key)).collect()
}

fn tally(votes: Vec<f64>) -> Ensemble {
    let mean = votes.iter().sum::<f64>() / votes.len().max(1) as f64;
    let max = votes.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let min = votes.iter().copied().fold(f64::INFINITY, f64::min);
    let spread = if votes.is_empty() { 0.0 } else { max - min };
    Ensemble {
        needs_human: spread >= NEEDS_HUMAN_SPREAD || mean < NEEDS_HUMAN_MEAN,
        votes,
        mean,
        spread,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn agreement_needs_no_human() {
        let e = tally(vec![0.9, 0.8, 0.85]);
        assert!(!e.needs_human);
        assert!((e.spread - 0.1).abs() < 1e-9);
    }

    #[test]
    fn split_or_negative_votes_need_a_human() {
        assert!(tally(vec![0.95, 0.2, 0.9]).needs_human);
        assert!(tally(vec![0.3, 0.4, 0.35]).needs_human);
    }
}
