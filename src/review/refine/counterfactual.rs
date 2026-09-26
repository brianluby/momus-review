//! Counterfactual calibration (#14): "what single fact would exonerate this?"
//! then "does the visible context show that fact?".
//!
//! The first question picks from a curated vocabulary (`EXONERATING_FACTS`);
//! Jev has no free-text generation. `none` ends the check. The second is a
//! skeptical `noul` against the surrounding code. At `EXONERATION_DROP` the
//! finding is dropped; otherwise the fact and its probability are kept so a
//! reviewer knows exactly what to check.

use futures::{StreamExt, stream};
use serde_json::json;

use crate::domain::policy::{EXONERATING_FACTS, EXONERATION_DROP, MAX_COUNTERFACTUAL};
use crate::domain::report::{Exoneration, Finding};
use crate::review::refine::{Refiner, finding_state, top_indices};
use crate::review::typesafe::{choice, choice_criteria, noul};

const NONE: &str = "none";

/// Checks the top `MAX_COUNTERFACTUAL` findings; returns the survivors and how
/// many were exonerated.
pub async fn calibrate(r: &Refiner<'_>, mut findings: Vec<Finding>) -> (Vec<Finding>, usize) {
    let targets = top_indices(&findings, MAX_COUNTERFACTUAL, |_| true);
    let findings_ref: &[Finding] = &findings;
    let results: Vec<(usize, anyhow::Result<Exoneration>)> = stream::iter(targets)
        .map(|i| async move { (i, check(r, &findings_ref[i]).await) })
        .buffered(r.concurrency)
        .collect()
        .await;

    for (i, result) in results {
        match result {
            Ok(exoneration) => findings[i].exoneration = Some(exoneration),
            Err(err) => (r.log)(&format!(
                "  counterfactual {}:{} failed (kept): {err:#}",
                findings[i].file, findings[i].line
            )),
        }
    }

    let before = findings.len();
    findings.retain(|f| !exonerated(f));
    let dropped = before - findings.len();
    (findings, dropped)
}

fn exonerated(finding: &Finding) -> bool {
    finding
        .exoneration
        .as_ref()
        .is_some_and(|e| e.fact != NONE && e.holds >= EXONERATION_DROP)
}

async fn check(r: &Refiner<'_>, finding: &Finding) -> anyhow::Result<Exoneration> {
    let state = finding_state(r, finding);
    let pick = r
        .client
        .system_one(
            state.clone(),
            json!({
                "fact": choice(
                    json!({
                        "question": "Which single fact, if it were true, would make this finding a false positive?",
                        "inspect": "selectedEvidence",
                        "focus": "The most plausible exonerating fact for this specific concern",
                    }),
                    choice_criteria(&EXONERATING_FACTS),
                ),
            }),
        )
        .await?;
    let (fact, fact_confidence) = pick.choice("fact")?;
    if fact == NONE {
        return Ok(Exoneration { fact, fact_confidence, holds: 0.0 });
    }
    let description = EXONERATING_FACTS
        .iter()
        .find(|(key, _)| *key == fact)
        .map(|(_, d)| *d)
        .unwrap_or("");

    let verdict = r
        .client
        .system_one(
            state,
            json!({
                "holds": noul(
                    json!({
                        "question": format!("Does fileContext or neighbors concretely establish this fact for the concern: {description}?"),
                        "inspect": ["fileContext", "neighbors", "selectedEvidence"],
                        "caution": "Absence of evidence is not proof; answer true only when the visible code shows the fact",
                    }),
                    json!({
                        "true": { "what": "The visible code concretely establishes the fact" },
                        "false": { "what": "The fact is not shown, or the code contradicts it" },
                    }),
                ),
            }),
        )
        .await?;
    Ok(Exoneration { fact, fact_confidence, holds: verdict.noul("holds")? })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with(fact: &str, holds: f64) -> Finding {
        Finding {
            exoneration: Some(Exoneration { fact: fact.into(), fact_confidence: 0.9, holds }),
            ..Default::default()
        }
    }

    #[test]
    fn only_a_shown_fact_exonerates() {
        assert!(exonerated(&with("unreachable", EXONERATION_DROP)));
        assert!(!exonerated(&with("unreachable", EXONERATION_DROP - 0.01)));
        assert!(!exonerated(&with(NONE, 1.0)));
        assert!(!exonerated(&Finding::default()));
    }

    #[test]
    fn vocabulary_ends_with_none() {
        assert_eq!(EXONERATING_FACTS.last().unwrap().0, NONE);
    }
}
