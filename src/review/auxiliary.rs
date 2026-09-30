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
    for mut finding in new_findings {
        finding.fingerprint = fingerprint(&finding);
        if suppressed.contains(&finding.fingerprint) {
            report.workflow.suppressed_findings += 1;
        } else {
            report.findings.push(finding);
        }
    }
    report.findings.sort_by(|a, b| {
        b.severity
            .total_cmp(&a.severity)
            .then(a.file.cmp(&b.file))
            .then(a.line.cmp(&b.line))
    });
    report.p_revert = super::merge_confidence::p_revert(&report.findings, &report.matrix);
    report.reviewed_head = Some(evidence.head);
}
