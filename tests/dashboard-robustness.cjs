// Dependency-free render regressions for incomplete reviews and applied thresholds.
const assert = require("node:assert/strict");
const test = require("node:test");
const vm = require("node:vm");
const fs = require("node:fs");
const path = require("node:path");

class Element {
  constructor(tag) { this.tag = tag; this.children = []; this.attrs = {}; this.style = { setProperty() {} }; }
  setAttribute(k, v) { this.attrs[k] = v; }
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
