//! Meta-judge false-positive filter: a second, skeptical pass after mechanism
//! classification that drops located findings whose evidence does not
//! concretely support the mechanism. Mirrors `review/meta.ts`.

use anyhow::Result;
use serde_json::{json, Value};

use crate::domain::policy::Dimension;
use crate::review::typesafe::{noul, TypeSafeClient};

/// Re-screens one located concern, independently of the earlier screening:
/// returns the probability that `selectedEvidence` concretely supports that
/// `mechanism` is the concern for `dimension` (rather than a weaker or
/// unrelated issue). Callers drop the finding below `MIN_META_JUDGE_CONFIDENCE`.
pub async fn judge(
    client: &TypeSafeClient,
    dimension: Dimension,
    mechanism: &str,
    evidence: &Value,
) -> Result<f64> {
    let state = json!({
        "suspectedConcern": {
            "dimension": dimension,
            "definition": dimension.definition(),
            "mechanism": mechanism,
        },
        "selectedEvidence": evidence,
    });

    let question = format!(
        "Independently of the earlier screening, does selectedEvidence concretely support that this is a '{mechanism}' concern, rather than a weaker or unrelated issue?"
    );

    let questions = json!({
        "supported": noul(
            json!({ "question": question, "inspect": "selectedEvidence" }),
            json!({
                "true": { "what": "The evidence concretely shows the mechanism" },
                "false": { "what": "The evidence is too weak, or supports a different or lesser issue" },
            }),
        ),
    });

    let response = client.system_one(state, questions).await?;
    response.noul("supported")
}