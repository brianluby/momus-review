//! Merge validated partial scans and run all global refinement stages once.
use super::{
    codebase::CodebaseStrategy,
    strategy::{FileEntry, ReviewStrategy},
};
use crate::domain::report::{ReviewMode, ReviewReport, ReviewStage};
use anyhow::{Result, bail};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

pub async fn merge_reports(
    parts: Vec<ReviewReport>,
    scopes: &[PathBuf],
    strategy: CodebaseStrategy,
    log: &dyn Fn(&str),
) -> Result<ReviewReport> {
    let (count, key, expected, refine, model, config) = validate(&parts)?;
    let discovery = strategy.discover(scopes)?;
    if super::planner::inventory_key(&discovery.files, &discovery.context_files) != key {
        bail!("merge checkout differs from the scanned inventory (including dirty files/tests)");
    }
    let merge_model = strategy.client().model_identity();
    if parts
        .iter()
        .any(|p| !p.matrix.is_empty() || p.tier.screened > 0)
        && merge_model != model
    {
        bail!(
            "merge must use the shards' pinned model and server: expected {model}, got {merge_model}"
        );
    }
    strategy.prepass(scopes, &discovery)?;
    let files: HashMap<&str, &_> = discovery.files.iter().map(|f| (f.path(), f)).collect();
    let mut report = ReviewReport {
        mode: ReviewMode::Codebase,
        scope: scopes
            .iter()
            .map(|s| s.display().to_string())
            .collect::<Vec<_>>()
            .join(" "),
        config,
        dimensions: crate::domain::policy::dimension_metadata(),
        ..Default::default()
    };
    let mut seen_files = HashSet::new();
    for part in parts {
        let shard = part.shard.as_ref().expect("validated metadata");
        let membership = super::planner::Shard {
            index: shard.index,
            count,
        };
        for row in &part.matrix {
            if !membership.contains(&row.file)
                || !expected.contains(&row.file)
                || !seen_files.insert(row.file.clone())
            {
                bail!(
                    "duplicate or wrongly assigned file {} in shard {}",
                    row.file,
                    shard.index
                );
            }
        }
        for path in &part.tier.dismissed {
            if !membership.contains(path)
                || !expected.contains(path)
                || !seen_files.insert(path.clone())
            {
                bail!("duplicate or wrongly assigned tier dismissal: {path}");
            }
        }
        for skipped in &part.skipped {
            if skipped.stage == ReviewStage::Screen
                && (!membership.contains(&skipped.file) || !expected.contains(&skipped.file))
            {
                bail!("wrongly assigned failed screen: {}", skipped.file);
            }
        }
        for finding in &part.findings {
            if !membership.contains(&finding.file) || !seen_files.contains(&finding.file) {
                bail!("finding outside its shard matrix: {}", finding.file);
            }
        }
        report.partial |= !part.skipped.is_empty();
        report.matrix.extend(part.matrix);
        report.findings.extend(part.findings);
        report.profiles.extend(part.profiles);
        report.context_files.extend(part.context_files);
        report.skipped.extend(part.skipped);
        report.usage.calls += part.usage.calls;
        report.usage.input_tokens += part.usage.input_tokens;
        report.usage.output_tokens += part.usage.output_tokens;
        report.usage.cache.hits += part.usage.cache.hits;
        report.usage.cache.misses += part.usage.cache.misses;
        report.budget.reserved += part.budget.reserved;
        report.budget.deferred += part.budget.deferred;
        report.index.computed += part.index.computed;
        report.index.reused += part.index.reused;
        report.index.fallbacks += part.index.fallbacks;
        report.tier.screened += part.tier.screened;
        report.tier.dismissed.extend(part.tier.dismissed);
        for (rule, n) in part.redactions {
            *report.redactions.entry(rule).or_default() += n;
        }
        let w = &mut report.workflow;
        let p = part.workflow;
        w.screened_cells += p.screened_cells;
        w.threshold_signals += p.threshold_signals;
        w.profiled_files += p.profiled_files;
        w.followed_signals += p.followed_signals;
        w.located_findings += p.located_findings;
        w.routed_findings += p.routed_findings;
        w.suppressed_findings += p.suppressed_findings;
        w.dropped_context_chars += p.dropped_context_chars;
        w.dropped_context_items += p.dropped_context_items;
    }
    report.matrix.sort_by(|a, b| a.file.cmp(&b.file));
    let probability = |path: &str| {
        report
            .matrix
            .iter()
            .find(|r| r.file == path)
            .map(|r| r.probabilities.values().copied().fold(0.0, f64::max))
            .unwrap_or(0.0)
    };
    report.profiles.sort_by(|a, b| {
        probability(&b.file)
            .total_cmp(&probability(&a.file))
            .then(a.file.cmp(&b.file))
    });
    report
        .profiles
        .truncate(crate::domain::policy::MAX_PROFILES);
    report.workflow.profiled_files = report.profiles.len();
    report.context_files.sort();
    report.context_files.dedup();
    report.tier.dismissed.sort();
    report.tier.dismissed.dedup();
    report.partial |= report.budget.deferred > 0 || !report.tier.dismissed.is_empty();
    report.screened_files = report.matrix.len();
    report.followed_signals = report.workflow.followed_signals;
    // Every expected path must have a screen, an explicit Tier-0 dismissal,
    // or an explicit failed/deferred screen; omission cannot look complete.
    for path in expected {
        if !seen_files.contains(&path)
            && !report.tier.dismissed.contains(&path)
            && !report
                .skipped
                .iter()
                .any(|s| s.stage == ReviewStage::Screen && s.file == path)
        {
            bail!("shards omitted expected file {path}");
        }
    }
    log(&format!(
        "Merging {count} shards ({model}): {} files",
        report.screened_files
    ));
    let (findings, counts) = super::workflow::finish_findings(
        &strategy,
        &files,
        report.findings,
        refine,
        log,
        crate::domain::policy::CONCURRENCY,
    )
    .await;
    report.findings = findings;
    report.workflow.routed_findings = report.findings.iter().filter(|f| f.owner.is_some()).count();
    report.workflow.clustered_findings = counts.clustered;
    report.workflow.exonerated_findings = counts.exonerated;
    report.workflow.needs_human_findings = counts.needs_human;
    report.p_revert = super::merge_confidence::p_revert(&report.findings, &report.matrix);
    let usage = strategy.client().usage_summary();
    report.usage.calls += usage.calls;
    report.usage.input_tokens += usage.input_tokens;
    report.usage.output_tokens += usage.output_tokens;
    report.usage.cache.hits += usage.cache.hits;
    report.usage.cache.misses += usage.cache.misses;
    let budget = strategy.client().budget_summary();
    report.budget.reserved += budget.reserved;
    report.budget.deferred += budget.deferred;
    report.partial |= budget.deferred > 0;
    for (rule, n) in strategy.client().redaction_summary() {
        *report.redactions.entry(rule).or_default() += n;
    }
    Ok(report)
}

type Validation = (
    usize,
    String,
    HashSet<String>,
    bool,
    String,
    crate::domain::report::ConfigSnapshot,
);
fn validate(parts: &[ReviewReport]) -> Result<Validation> {
    let Some(first) = parts.first() else {
        bail!("merge needs at least one partial report");
    };
    let Some(meta) = &first.shard else {
        bail!("merge accepts partial shard reports only");
    };
    if meta.count == 0 || meta.count > 256 || parts.len() != meta.count {
        bail!(
            "missing shards: expected {}, got {}",
            meta.count,
            parts.len()
        );
    }
    let expected: HashSet<_> = meta.expected_paths.iter().cloned().collect();
    if expected.len() != meta.expected_paths.len() {
        bail!("duplicate paths in shard inventory");
    }
    let mut seen = HashSet::new();
    let config = serde_json::to_value(&first.config)?;
    let model = parts
        .iter()
        .find(|p| !p.matrix.is_empty() || p.tier.screened > 0)
        .and_then(|p| p.shard.as_ref())
        .map_or(&meta.model, |m| &m.model);
    for part in parts {
        let Some(m) = &part.shard else {
            bail!("non-shard report in merge");
        };
        if part.mode != ReviewMode::Codebase
            || m.count != meta.count
            || m.index == 0
            || m.index > m.count
            || !seen.insert(m.index)
            || m.inventory_key != meta.inventory_key
            || m.refine != meta.refine
            || m.expected_paths.iter().cloned().collect::<HashSet<_>>() != expected
            || serde_json::to_value(&part.config)? != config
        {
            bail!("incompatible or duplicate shard report");
        }
        // Empty shards have made no model requests; do not reject their unresolved aliases.
        if (!part.matrix.is_empty() || part.tier.screened > 0) && &m.model != model {
            bail!("shards used different model versions");
        }
    }
    Ok((
        meta.count,
        meta.inventory_key.clone(),
        expected,
        meta.refine,
        model.clone(),
        first.config.clone(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_duplicate_and_mixed_shards_are_rejected() {
        let mk = |index| ReviewReport {
            mode: ReviewMode::Codebase,
            shard: Some(crate::domain::report::ShardMetadata {
                index,
                count: 2,
                inventory_key: "a".into(),
                expected_paths: vec!["x".into()],
                refine: false,
                model: "stub".into(),
            }),
            ..Default::default()
        };
        assert!(validate(&[mk(1)]).is_err());
        assert!(validate(&[mk(1), mk(1)]).is_err());
        let mut changed = mk(2);
        changed.shard.as_mut().unwrap().inventory_key = "b".into();
        assert!(validate(&[mk(1), changed]).is_err());
        assert!(validate(&[mk(2), mk(1)]).is_ok());
    }
}
