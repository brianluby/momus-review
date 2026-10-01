//! Local evidence checks join the standard finding and suppression flow.
use crate::{
    adapters::git::RepositoryEvidence,
    domain::{
        feedback::{FeedbackLog, fingerprint},
        report::ReviewReport,
    },
};
#[derive(Debug)]
pub struct AuxiliaryInputs {
    pub evidence: RepositoryEvidence,
    pub upgrades: bool,
    pub docs_drift: bool,
}
pub fn attach(report: &mut ReviewReport, inputs: AuxiliaryInputs, feedback: &FeedbackLog) {
    let mut evidence = inputs.evidence;
    match &report.reviewed_head {
        Some(head) if *head != evidence.head => {
            report.partial = true;
            report.reviewed_clean = false;
            report.reviewed_committed = false;
            evidence.unknowns.push("Auxiliary evidence head differs from the source review head; combined evidence identity is unverified".into());
        }
        None => {
            report.partial = true;
            report.reviewed_clean = false;
            report.reviewed_committed = false;
            evidence.unknowns.push("Source review head was not captured; auxiliary evidence cannot establish source identity".into());
        }
        _ => {}
    }
    let mut redactions = crate::domain::redact::Redactions::default();
    for file in &mut evidence.files {
        file.content = crate::domain::redact::redact(&file.content, &mut redactions);
    }
    for change in &mut evidence.changes {
        change.base = crate::domain::redact::redact(&change.base, &mut redactions);
        if let Some(text) = &mut change.content {
            *text = crate::domain::redact::redact(text, &mut redactions);
        }
    }
    if !redactions.rules().is_empty() {
        evidence.unknowns.push("Secrets were redacted before local comparison; differences within credential values cannot be assessed".into());
    }
    let suppressed = feedback.suppressed();
    let mut new_findings = Vec::new();
    if inputs.upgrades {
        let mut summary = super::upgrades::assess(&evidence.changes, &evidence.files);
        summary.unknowns.extend(evidence.unknowns.iter().cloned());
        report.partial |= !summary.unknowns.is_empty();
        new_findings.extend(std::mem::take(&mut summary.findings));
        report.upgrades = Some(summary);
    }
    if inputs.docs_drift {
        let mut summary = super::docs_drift::assess(&evidence.changes, &evidence.files);
        summary.unknowns.extend(evidence.unknowns.iter().cloned());
        report.partial |= !summary.unknowns.is_empty();
        new_findings.extend(std::mem::take(&mut summary.findings));
        report.docs_drift = Some(summary);
    }
    new_findings.sort_by(|a, b| {
        b.severity
            .total_cmp(&a.severity)
            .then(a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
    });
    for mut finding in new_findings {
        finding.fingerprint = fingerprint(&finding);
        if suppressed.contains(&finding.fingerprint) {
            report.workflow.suppressed_findings += 1;
        } else {
            report.findings.push(finding);
        }
    }
    // Keep the source refinement/pairwise ranking intact; only appended
    // advisory findings are sorted locally.
    report.p_revert = super::merge_confidence::p_revert(&report.findings, &report.matrix);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auxiliary_head_cannot_overwrite_verified_source_identity() {
        let mut report = ReviewReport {
            reviewed_head: Some("source-head".into()),
            reviewed_clean: true,
            reviewed_committed: true,
            ..Default::default()
        };
        attach(
            &mut report,
            AuxiliaryInputs {
                evidence: RepositoryEvidence {
                    head: "other-head".into(),
                    ..Default::default()
                },
                upgrades: true,
                docs_drift: false,
            },
            &FeedbackLog::default(),
        );
        assert_eq!(report.reviewed_head.as_deref(), Some("source-head"));
        assert!(!report.reviewed_clean && !report.reviewed_committed);
        assert!(report.partial);
        assert!(
            report
                .upgrades
                .unwrap()
                .unknowns
                .iter()
                .any(|s| s.contains("head differs"))
        );
    }

    #[test]
    fn auxiliary_head_cannot_fill_missing_source_identity() {
        let mut report = ReviewReport {
            reviewed_clean: true,
            reviewed_committed: true,
            ..Default::default()
        };
        attach(
            &mut report,
            AuxiliaryInputs {
                evidence: RepositoryEvidence {
                    head: "auxiliary-head".into(),
                    ..Default::default()
                },
                upgrades: true,
                docs_drift: false,
            },
            &FeedbackLog::default(),
        );
        assert!(report.reviewed_head.is_none());
        assert!(report.partial && !report.reviewed_clean && !report.reviewed_committed);
        assert!(
            report
                .upgrades
                .unwrap()
                .unknowns
                .iter()
                .any(|s| s.contains("head was not captured"))
        );
    }

    #[test]
    fn advisories_preserve_primary_rank_order_and_disclose_redacted_comparisons() {
        use crate::domain::{report::Finding, repository::RepositoryChange};
        let mut report = ReviewReport {
            reviewed_head: Some("source-head".into()),
            findings: vec![
                Finding {
                    file: "z.rs".into(),
                    rank: Some(1),
                    ..Default::default()
                },
                Finding {
                    file: "a.rs".into(),
                    rank: Some(2),
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        attach(&mut report, AuxiliaryInputs {
            evidence: RepositoryEvidence {
                head: "source-head".into(),
                changes: vec![RepositoryChange {
                    path: "Cargo.toml".into(), old_path: None,
                    base: "[dependencies]\na='1.0.0'\nb={git='https://user:old-secret@host/repo'}\n".into(),
                    content: Some("[dependencies]\na='2.0.0'\nb={git='https://user:new-secret@host/repo'}\n".into()),
                }],
                ..Default::default()
            }, upgrades: true, docs_drift: false,
        }, &FeedbackLog::default());
        assert_eq!(report.findings[0].rank, Some(1));
        assert_eq!(report.findings[1].rank, Some(2));
        assert_eq!(report.findings[0].file, "z.rs");
        assert!(report.findings.len() > 2);
        assert!(report.partial);
        assert!(
            report
                .upgrades
                .unwrap()
                .unknowns
                .iter()
                .any(|s| s.contains("credential values cannot be assessed"))
        );
    }
}
