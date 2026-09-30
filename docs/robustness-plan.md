# Review robustness implementation plan

Tickets #18, #19 and #11 share the existing redacted, content-addressed System
One client. They add no dependency and preserve the default review behavior.
Implementation starts from origin/main at 89f5359 on feat/review-robustness.

## #18: Test planner

Opt in with `--test-plans` in review or scan. For up to eight well-supported
test-gap findings after refinement, choose a compatible test strategy with
an explicit abstention option. Carry source, neighboring and existing-test
context. Save the selected strategy, confidence, scenario, assertion guidance
and deliberately unfinished language-specific scaffold on the finding.
Never apply or execute generated scaffolds, invent repository APIs, or claim
that a scaffold provides coverage. Expose plans in JSON, the dashboard and PR
comments. Invalid or uncertain choices leave the finding intact without a plan.

## #19: Spec drift

Opt in with repeatable `--spec PATH` local UTF-8 requirements files. Reject
empty, excessive or non-regular input; retain source path, line and content hash
provenance. Compare each reviewed file with the supplied requirements using
bounded context and typed judgments. A contradiction requires confident
selection of existing requirement/source evidence and a skeptical confirmation.
Keep matches, uncertainty, irrelevance and deferred checks distinct. Results
are advisory, separate from the five-dimension defect matrix and blocking gate;
surface them in JSON, the dashboard and the PR summary. One contradiction per
file is a discovery limit, not proof of complete requirements coverage.

## #11: Value-of-information follow-up budgets

Opt in with `--follow-up-strategy voi`. Rank threshold-eligible signals by a
documented heuristic using concern probability, an uncertainty bonus, dimension
impact and nominal call cost. Preserve dimension representation when a follow-up
cap permits it; unlimited mode retains all eligible signals. Save the ranking
inputs and selection decisions. Add repeatable `--threshold DIMENSION=P` overrides
after feedback tuning. Existing token/call/cache/wall-time meters report actual
usage. The atomic HTTP-attempt budget remains authoritative, including retries;
estimated costs never reject cache-only work. With a finite call cap, VOI omits
optional profiles and completes one locate pipeline at a time before the next.
This is an auditable heuristic, not calibrated incident probability or dollar
savings. Deliberately omitted follow-ups mark coverage partial.

## Ownership and integration

Three parallel lanes own test_planner.rs, spec_drift.rs, and voi.rs/workflow.rs.
The coordinating agent owns shared report types, CLI, renderer/publisher wiring,
documentation, integration tests and Git publication. Existing sharded workflows
remain supported; these new global options initially reject `--shard` explicitly
to avoid per-shard caps or missing global artifact provenance.

## Acceptance checks

- Default invocations make no new planner/spec calls and retain report compatibility.
- Strategy choices abstain on unsupported, malformed and low-confidence answers;
  raw evidence never becomes executable scaffold text.
- Spec checks use exact evidence anchors, redact secrets on the wire and in
  publication, reject truncated authority, and report unfinished work as partial.
- VOI selection is deterministic, retains severe likely concerns, respects
  dimension thresholds/caps and permits warm cache replay with zero HTTP budget.
- Child-process CLI tests use mock HTTP to verify combined flags, wire context,
  output, cold/warm caching, spec changes and hard budget exhaustion.
- Run all-target/all-feature tests, strict Clippy, build and formatting checks;
  review the combined diff before publication.
