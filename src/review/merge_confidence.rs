//! Merge-confidence (P(revert)) spike (#4).
//!
//! Produces a scalar in `[0, 1)` summarizing how risky a merge looks from the
//! review signals alone. This is an **uncalibrated heuristic spike** — the
//! weights are hand-picked, not fitted, and the value must NOT be presented
//! or treated as a calibrated probability. Roadmap item #17 owns the real
//! calibration against observed revert outcomes.
//!
//! The heuristic is a logistic squash of a weighted `risk` score:
//!
//! ```text
//! blocking      = count of findings whose action == RequestChanges
//! max_severity  = max finding.severity (0.0 when there are no findings)
//! max_test_gap  = max row.probabilities[TestGap] over the matrix
//!                 (0.0 when absent)
//! risk = 0.55 * min(1, blocking)
//!      + 0.25 * (max_severity / SEVERITY_MAX)
//!      + 0.10 * max_test_gap
//!      + 0.10 * min(1, matrix.len() / 50)
//! estimate = 1 - exp(-3 * risk)
//! ```
//!
//! With no findings and an empty matrix, `risk` is 0 and the estimate is 0;
//! the exponential term keeps the result asymptotically below 1 no matter how
//! large `risk` grows.

use crate::domain::policy::{Dimension, SEVERITY_MAX};
use crate::domain::report::{Action, Finding, MatrixRow};

/// Uncalibrated P(revert) heuristic in `[0, 1)`. See the module doc for the
/// formula; calibration is owned by #17.
pub fn estimate(findings: &[Finding], matrix: &[MatrixRow]) -> f64 {
    let blocking = findings
        .iter()
        .filter(|f| f.action == Action::RequestChanges)
        .count() as f64;

    let max_severity = findings.iter().map(|f| f.severity).fold(0.0, f64::max);

    let max_test_gap = matrix
        .iter()
        .filter_map(|row| row.probabilities.get(&Dimension::TestGap).copied())
        .fold(0.0, f64::max);

    let risk = 0.55 * blocking.min(1.0)
        + 0.25 * (max_severity / SEVERITY_MAX)
        + 0.10 * max_test_gap
        + 0.10 * (matrix.len() as f64 / 50.0).min(1.0);

    1.0 - (-3.0 * risk).exp()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    fn finding_with(action: Action, severity: f64) -> Finding {
        Finding {
            file: "src/main.rs".into(),
            line: 1,
            dimension: Dimension::TestGap,
            probability: 0.93,
            location_confidence: 0.9,
            mechanism: "boundary".into(),
            mechanism_confidence: 0.5,
            severity,
            severity_confidence: 0.5,
            owner: None,
            owner_confidence: None,
            action,
            evidence: "an excerpt".into(),
            title: None,
            why: None,
            fix: None,
            test: None,
        }
    }

    fn matrix_row_with(test_gap: f64) -> MatrixRow {
        MatrixRow {
            file: "src/main.rs".into(),
            probabilities: BTreeMap::from([(Dimension::TestGap, test_gap)]),
        }
    }

    #[test]
    fn empty_is_zero() {
        assert!((estimate(&[], &[]) - 0.0).abs() < 1e-9);
    }

    #[test]
    fn blocking_increases_p() {
        let empty = estimate(&[], &[]);
        let one = estimate(&[finding_with(Action::RequestChanges, 1.0)], &[]);
        let two = estimate(
            &[
                finding_with(Action::RequestChanges, 1.0),
                finding_with(Action::RequestChanges, 3.0),
            ],
            &[],
        );
        assert!(one > empty, "a blocking finding must raise the estimate");
        assert!(two > one, "a second blocking finding raises it further");
    }

    #[test]
    fn estimate_is_bounded() {
        let findings = (0..5)
            .map(|_| finding_with(Action::RequestChanges, SEVERITY_MAX))
            .collect::<Vec<_>>();
        let matrix = (0..60).map(|_| matrix_row_with(1.0)).collect::<Vec<_>>();
        let p = estimate(&findings, &matrix);
        assert!(p >= 0.0);
        assert!(p < 1.0);
    }
}