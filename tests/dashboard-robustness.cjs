// Dependency-free render regressions for incomplete reviews and applied thresholds.
const assert = require("node:assert/strict");
const test = require("node:test");
const vm = require("node:vm");
const fs = require("node:fs");
const path = require("node:path");

class Element {
  constructor(tag) { this.tag = tag; this.children = []; this.attrs = {}; this.style = { setProperty() {} }; }
  setAttribute(k, v) { this.attrs[k] = v; }
  set innerHTML(_) { throw new Error("Untrusted evidence must use text nodes"); }
  insertAdjacentHTML() { throw new Error("Untrusted evidence must use text nodes"); }
  addEventListener() {}
  append(child) { this.children.push(child); }
  replaceChildren(...children) { this.children = children; }
  get textContent() { return this.children.map((c) => c instanceof Element ? c.textContent : String(c)).join(" "); }
}

function dashboard() {
  const document = {
    createElement: (tag) => new Element(tag),
    getElementById: () => new Element("div"),
    addEventListener() {}, body: { dataset: {} },
  };
  const context = vm.createContext({ document, Node: Element, window: { addEventListener() {} }, fetch: async () => ({ok:false}), console });
  vm.runInContext(fs.readFileSync(path.join(__dirname, "../src/dashboard/public/app.js"), "utf8"), context);
  return context;
}

function elements(root) { return [root, ...root.children.filter((c) => c instanceof Element).flatMap(elements)]; }

test("capped empty review prominently discloses incomplete coverage", () => {
  const ctx = dashboard();
  ctx.report = { partial:true, findings:[], followedSignals:0, workflow:{thresholdSignals:2, followedSignals:0}, budget:{deferred:1}, specDrift:{checks:[{status:"uncertain"}]} };
  const banner = vm.runInContext("partialCoverage(report)", ctx);
  assert.match(banner.textContent, /coverage is incomplete/);
  assert.match(banner.textContent, /2 follow-ups omitted/);
  assert.match(banner.textContent, /1 uncertain\/deferred spec checks/);
  const empty = vm.runInContext("findings(report)", ctx);
  assert.match(empty.textContent, /Review incomplete/);
  assert.doesNotMatch(empty.textContent, /No screening probability reached/);
});

test("matrix highlights using each applied threshold and renders evidence as text", () => {
  const ctx = dashboard();
  ctx.report = { config:{screenThresholds:{security:0.6, testGap:0.9}}, matrix:[{file:"a.rs", security:0.65, testGap:0.85}] };
  const matrix = vm.runInContext("matrix(report)", ctx);
  const cells = elements(matrix).filter((e) => e.tag === "td" && e.attrs.title);
  assert.equal(cells.find((e) => e.attrs.title.includes("Security")).className.includes("hot"), true);
  assert.equal(cells.find((e) => e.attrs.title.includes("Test gap")).className.includes("hot"), false);
  assert.match(matrix.textContent, /Security 0.60/);
  ctx.report = { specDrift:{checks:[{file:"a.rs",status:"drift",confidence:0.9,source:{path:"a.rs",startLine:1,endLine:1,text:"<script>throw 'injected'</script>"}}]} };
  const spec = vm.runInContext("specDrift(report)", ctx);
  assert.match(spec.textContent, /<script>/);
  assert.equal(elements(spec).some((e) => e.tag === "script"), false);
});


function renderLocal(report) {
  const ctx = dashboard();
  ctx.report = report;
  return vm.runInContext("localAnalyses(report)", ctx);
}

// The field names and nesting below match DependencyChange, DocsCheck,
// MergeConfidenceSummary, OutcomeEstimate, Evaluation and ApprovalDecision.
function serializedAnalyses() {
  return {
    reviewedHead: "fixture-head", reviewedClean: true, reviewedCommitted: true,
    upgrades: { findings: [], unknowns: ["missing intervening changelog"], changes: [{
      ecosystem: "cargo", dependency: "x", scope: "build-dependencies/x", kind: "changed",
      oldVersion: "1.0.0", newVersion: "2.0.0", risk: "majorChange",
      evidence: [{ path: "Cargo.toml", line: 3, snapshot: "current", text: "x = '2.0.0' <script>unsafe()</script>" }],
      changelog: [{ path: "vendor/x/CHANGELOG.md", line: 5, snapshot: "current", text: "Removed old API" }],
    }] },
    docsDrift: { findings: [], unknowns: ["ambiguous interface"], checks: [{
      status: "drift", symbol: "removed_api", reason: "Documented interface was removed",
      documentation: { path: "README.md", line: 2, revision: "current", text: "removed_api() <img src=x onerror=unsafe()>" },
      source: { path: "src/old.rs", line: 7, revision: "base", text: "pub fn removed_api() {}" },
    }] },
    mergeConfidence: {
      version: 1, heuristicScore: 0.34, heuristicLabel: "routine", repository: "owner/repo",
      provenance: "fixture <script>unsafe()</script>", synthetic: true, trainingCutoff: 100, asOf: 200,
      outcomes: [{ outcome: "revert", probability: 0.1, status: "syntheticDemonstration", windowSeconds: 86400,
        upperBound95: 0.2, heldOutUpperBound95: 0.3, matchingBinSamples: 50,
        evaluation: { trainingSamples: 100, heldOutSamples: 20, unknownLabels: 4, immatureOrUnavailableLabels: 3,
          trainingEvents: 2, heldOutEvents: 1, brierScore: 0.05, baselineBrierScore: 0.07,
          expectedCalibrationError: 0.02, matchingBinHeldOutSamples: 10, matchingBinHeldOutEvents: 1, matchingBinCalibrationError: 0.01 },
        limitations: ["Synthetic fixtures do not establish real-world calibration"],
      }],
      approval: { enabled: true, eligible: false, reasons: ["synthetic history cannot authorize approval"] },
    },
  };
}

test("serializer-shaped local analyses label provenance and preserve inert evidence", () => {
  const rendered = renderLocal(serializedAnalyses());
  const text = rendered.textContent;
  assert.match(text, /cargo · x · build-dependencies\/x · changed: 1\.0\.0 → 2\.0\.0 · major change/);
  assert.match(text, /Manifest \/ lock evidence:.*Current snapshot.*Cargo\.toml:3/);
  assert.match(text, /Changelog evidence:.*Current snapshot.*vendor\/x\/CHANGELOG\.md:5/);
  assert.match(text, /removed_api: drift/);
  assert.match(text, /Reason: Documented interface was removed/);
  assert.match(text, /Documentation evidence:.*Current snapshot.*README\.md:2/);
  const historical = elements(rendered).find((element) => element.tag === "p" && element.textContent.includes("Source evidence:"));
  assert.match(historical.textContent, /Base snapshot \(pre-change\).*src\/old\.rs:7/);
  assert.doesNotMatch(historical.textContent, /Current snapshot/);
  assert.match(text, /Unknown: missing intervening changelog/);
  assert.match(text, /Unknown: ambiguous interface/);
  assert.match(text, /<script>/);
  assert.match(text, /<img src=x onerror=unsafe\(\)>/);
  assert.equal(elements(rendered).some((element) => ["script", "img"].includes(element.tag)), false);
  assert.doesNotMatch(text, /"documentation"\s*:|"revision"\s*:|undefined/);
});

test("outcomes disclose synthetic history, sampling bounds, evaluation limits and approval binding", () => {
  const text = renderLocal(serializedAnalyses()).textContent;
  assert.match(text, /History: synthetic demonstration — not real-world calibration/);
  assert.match(text, /Repository: owner\/repo.*Provenance: fixture/);
  assert.match(text, /Training cutoff: 100.*Assessed as of: 200/);
  assert.match(text, /Reviewed head: fixture-head · Clean checkout verified: yes · Committed review verified: yes/);
  assert.match(text, /revert: 10\.00% · synthetic demonstration/);
  assert.match(text, /Observation window: 86400 seconds/);
  assert.match(text, /95% upper sampling bound \(training bin\): 20\.00% · Held-out bin: 30\.00%/);
  assert.match(text, /Sampling bounds do not bound distribution shift/);
  assert.match(text, /100 training \/ 20 held out · Unknown labels: 4 · Immature or unavailable labels: 3/);
  assert.match(text, /Matching held-out bin: 10 samples \/ 1 events/);
  assert.match(text, /Held-out Brier score: 0\.05 · Baseline Brier: 0\.07/);
  assert.match(text, /Limitation: Synthetic fixtures do not establish real-world calibration/);
  assert.match(text, /Automatic approval: rejected under explicit policy · Policy enabled: yes · Eligible: no/);
  assert.match(text, /Approval reason: synthetic history cannot authorize approval/);
});

test("dependency additions and removals retain ecosystem, scope and nullable version meaning", () => {
  const report = serializedAnalyses();
  report.upgrades.changes = [
    { ...report.upgrades.changes[0], ecosystem: "npm", scope: "dependencies/x", kind: "added", oldVersion: null, newVersion: "1.2.3", risk: "newDependency" },
    { ...report.upgrades.changes[0], scope: "dependencies/x", kind: "removed", oldVersion: "1.0.0", newVersion: null, risk: "removal",
      evidence: [{ path: "Cargo.toml", line: 4, snapshot: "base", text: "x = '1.0.0'" }], changelog: [] },
  ];
  const text = renderLocal(report).textContent;
  assert.match(text, /npm · x · dependencies\/x · added: absent → 1\.2\.3 · new dependency/);
  assert.match(text, /cargo · x · dependencies\/x · removed: 1\.0\.0 → absent · removal/);
  assert.match(text, /Manifest \/ lock evidence:.*Base snapshot \(pre-change\).*Cargo\.toml:4/);
});

test("approval explicitly distinguishes eligible, rejected, disabled and unavailable states", () => {
  for (const [approval, state, enabled, eligible] of [
    [{ enabled: true, eligible: true, reasons: [] }, "eligible under explicit policy", "yes", "yes"],
    [{ enabled: true, eligible: false, reasons: [] }, "rejected under explicit policy", "yes", "no"],
    [{ enabled: false, eligible: false, reasons: [] }, "disabled", "no", "no"],
    [undefined, "unknown", "unknown", "unknown"],
  ]) {
    const text = renderLocal({ mergeConfidence: { outcomes: [], approval } }).textContent;
    assert.ok(text.includes(`Automatic approval: ${state} · Policy enabled: ${enabled} · Eligible: ${eligible}`));
  }
});

test("absent analyses produce no section and empty summaries disclose evidence limits", () => {
  assert.equal(renderLocal({}), null);
  const text = renderLocal({ upgrades: { changes: [], unknowns: [] }, docsDrift: { checks: [], unknowns: [] }, mergeConfidence: { outcomes: [] } }).textContent;
  assert.match(text, /No dependency changes were available.*compatibility is not established/);
  assert.match(text, /No documentation checks were available.*consistency is not established/);
  assert.match(text, /No outcome estimates were available; risk remains unknown/);
  assert.match(text, /Reviewed head: unknown · Clean checkout verified: unknown · Committed review verified: unknown/);
  assert.doesNotMatch(text, /undefined|0 training \/ 0 held out/);
});

test("missing evaluation fields remain unknown and actual zero counts remain visible", () => {
  const report = { mergeConfidence: { outcomes: [{ outcome: "incident", probability: null, status: "unknown" },
    { outcome: "flake", probability: 0, status: "evaluatedEmpirical", windowSeconds: 60, evaluation: { trainingSamples: 0, heldOutSamples: 0, unknownLabels: 0, immatureOrUnavailableLabels: 0 } }] } };
  const text = renderLocal(report).textContent;
  assert.match(text, /incident: unknown · unknown/);
  assert.match(text, /Evaluation: unknown training \/ unknown held out · Unknown labels: unknown · Immature or unavailable labels: unknown/);
  assert.match(text, /Observation window: unknown seconds/);
  assert.match(text, /flake: 0\.00% · evaluated empirical/);
  assert.match(text, /Evaluation: 0 training \/ 0 held out · Unknown labels: 0 · Immature or unavailable labels: 0/);
});

test("incomplete dependency and documentation evidence never renders undefined or a guessed revision", () => {
  const text = renderLocal({ upgrades: { changes: [{ evidence: [{}] }] }, docsDrift: { checks: [{ documentation: {}, source: null }] } }).textContent;
  assert.match(text, /Manifest \/ lock evidence:.*Snapshot unknown.*unknown:line unknown/);
  assert.match(text, /Changelog evidence: unavailable; breaking behavior remains unknown/);
  assert.match(text, /Documentation evidence:.*Snapshot unknown/);
  assert.match(text, /Source evidence: evidence unavailable/);
  assert.match(text, /unknown: unknown → unknown · unknown/);
  assert.doesNotMatch(text, /undefined|Current snapshot|Base snapshot/);
});

test("local analysis lists and evidence are bounded with exact omission notices", () => {
  const fixture = serializedAnalyses();
  fixture.upgrades.changes = Array.from({ length: 53 }, (_, index) => ({ ...fixture.upgrades.changes[0], dependency: `dependency-${index}`,
    evidence: Array.from({ length: 22 }, (_, entry) => ({ path: `manifest-${entry}`, line: 1, snapshot: "current", text: "literal evidence" })) }));
  fixture.docsDrift.checks = Array.from({ length: 52 }, (_, index) => ({ ...fixture.docsDrift.checks[0], symbol: `symbol-${index}` }));
  fixture.docsDrift.unknowns = Array.from({ length: 51 }, (_, index) => `docs-unknown-${index}`);
  const text = renderLocal(fixture).textContent;
  assert.match(text, /3 dependency changes omitted from this view/);
  assert.match(text, /2 manifest\/lock evidence entries omitted from this view/);
  assert.match(text, /2 documentation checks omitted from this view/);
  assert.match(text, /1 documentation unknowns omitted from this view/);
  assert.doesNotMatch(text, /dependency-50|manifest-20|symbol-50|docs-unknown-50/);
  const longEvidence = serializedAnalyses();
  longEvidence.upgrades.changes[0].evidence[0].text = "x".repeat(4000) + "omitted-secret-sentinel";
  const clipped = renderLocal(longEvidence).textContent;
  assert.match(clipped, /23 evidence characters omitted from this view/);
  assert.doesNotMatch(clipped, /omitted-secret-sentinel/);
});

test("heuristic risk uses a score scale instead of a probability percentage, including zero", () => {
  for (const [score, label] of [[0.34, "0.340 / 1"], [0, "0.000 / 1"]]) {
    const ctx = dashboard();
    ctx.report = { findings: [], pRevert: score };
    const rendered = vm.runInContext("summary(report)", ctx);
    assert.match(rendered.textContent, /risk score/);
    assert.ok(rendered.textContent.includes(label));
    assert.doesNotMatch(rendered.textContent, /%/);
    assert.match(elements(rendered).find((element) => element.attrs.title?.includes("hand-weighted")).attrs.title, /not a probability/);
  }
});
