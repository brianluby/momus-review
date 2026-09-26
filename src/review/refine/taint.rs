//! Multi-hop taint chains (#13) for injection-class security findings.
//!
//! Composed narrow calls instead of one broad "is this exploitable?":
//! 1. where does the data reaching the sink originate (`choice`), and does it
//!    actually reach the sink (`noul`) — one request;
//! 2. only when it plausibly reaches the sink: is it sanitized for that sink
//!    on the way (`noul`), asked with the identified source.
//!
//! `exploitability = untrusted × reaches_sink × (1 − sanitized)`. Below
//! `LOW_EXPLOITABILITY` a blocking finding is demoted to a comment; it is
//! never dropped, since the chain may run through code outside the context.

use futures::{StreamExt, stream};
use serde_json::json;

use crate::domain::policy::{
    Dimension, LOW_EXPLOITABILITY, MAX_TAINT, MIN_SINK_PROBABILITY, TAINT_MECHANISMS, TAINT_SOURCES,
    UNTRUSTED_SOURCES,
};
use crate::domain::report::{Action, Finding, TaintChain};
use crate::review::refine::{Refiner, finding_state, top_indices};
use crate::review::typesafe::{choice, choice_criteria, noul};

/// Traces the top `MAX_TAINT` injection-class findings in place.
pub async fn trace(r: &Refiner<'_>, findings: &mut [Finding]) {
    let targets = top_indices(findings, MAX_TAINT, |f| {
        f.dimension == Dimension::Security && TAINT_MECHANISMS.contains(&f.mechanism.as_str())
    });
    let findings_ref: &[Finding] = findings;
    let chains: Vec<(usize, anyhow::Result<TaintChain>)> = stream::iter(targets)
        .map(|i| async move { (i, chain(r, &findings_ref[i]).await) })
        .buffered(r.concurrency)
        .collect()
        .await;

    for (i, result) in chains {
        match result {
            Ok(chain) => {
                if chain.exploitability < LOW_EXPLOITABILITY && findings[i].action == Action::RequestChanges {
                    findings[i].action = Action::Comment;
                }
                findings[i].taint = Some(chain);
            }
            Err(err) => (r.log)(&format!(
                "  taint {}:{} failed (kept): {err:#}",
                findings[i].file, findings[i].line
            )),
        }
    }
}

async fn chain(r: &Refiner<'_>, finding: &Finding) -> anyhow::Result<TaintChain> {
    let state = finding_state(r, finding);
    let first = r
        .client
        .system_one(
            state.clone(),
            json!({
                "source": choice(
                    json!({
                        "question": "Where does the data that reaches the sink in selectedEvidence originate?",
                        "inspect": ["selectedEvidence", "fileContext", "neighbors"],
                        "focus": "Follow the value backwards through assignments, parameters, and callers",
                    }),
                    choice_criteria(&TAINT_SOURCES),
                ),
                "reachesSink": noul(
                    json!({
                        "question": "Does data in selectedEvidence actually reach the dangerous sink that concern.mechanismDescription names?",
                        "inspect": ["selectedEvidence", "fileContext"],
                    }),
                    json!({
                        "true": { "what": "A concrete data path reaches the dangerous sink" },
                        "false": { "what": "The data does not reach that sink, or no such sink is present" },
                    }),
                ),
            }),
        )
        .await?;
    let (source, source_confidence) = first.choice("source")?;
    let reaches_sink = first.noul("reachesSink")?;

    let sanitized = if reaches_sink < MIN_SINK_PROBABILITY {
        None
    } else {
        let second = r
            .client
            .system_one(
                state,
                json!({
                    "sanitized": noul(
                        json!({
                            "question": format!("Before it reaches the sink, is the data from source '{source}' validated, escaped, allow-listed, or parameterized adequately for that sink?"),
                            "inspect": ["selectedEvidence", "fileContext", "neighbors"],
                            "caution": "A defense for a different sink (e.g. HTML escaping before SQL) does not count",
                        }),
                        json!({
                            "true": { "what": "An adequate, sink-appropriate defense applies on every path" },
                            "false": { "what": "No defense, or one that is incomplete or wrong for this sink" },
                        }),
                    ),
                }),
            )
            .await?;
        Some(second.noul("sanitized")?)
    };

    let untrusted = untrusted(&source, source_confidence);
    Ok(TaintChain {
        source,
        source_confidence,
        untrusted,
        reaches_sink,
        sanitized,
        exploitability: untrusted * reaches_sink * (1.0 - sanitized.unwrap_or(0.0)),
    })
}

/// P(untrusted origin) from the source choice: its confidence for an
/// untrusted label, the complement for a trusted one, even odds if unknown.
fn untrusted(source: &str, confidence: f64) -> f64 {
    if UNTRUSTED_SOURCES.contains(&source) {
        confidence
    } else if source == "unknown" {
        0.5
    } else {
        1.0 - confidence
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn untrusted_maps_source_labels() {
        assert_eq!(untrusted("requestInput", 0.9), 0.9);
        assert!((untrusted("serverConfig", 0.9) - 0.1).abs() < 1e-9);
        assert_eq!(untrusted("unknown", 0.9), 0.5);
    }

    #[test]
    fn every_untrusted_source_is_in_the_vocabulary() {
        for s in UNTRUSTED_SOURCES {
            assert!(TAINT_SOURCES.iter().any(|(k, _)| *k == s), "{s}");
        }
    }
}
