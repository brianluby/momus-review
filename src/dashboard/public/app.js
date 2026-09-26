// Mirrors SCREEN_THRESHOLD in src/domain/config.ts: signals at or above it are followed.
const THRESHOLD = 0.7;
// Severity is an expected score on the 0–3 rubric in src/domain/config.ts.
const SEVERITY_MAX = 3;

const DEFAULT_DIMENSIONS = [
  ["correctness", "Correctness", "Corr"],
  ["security", "Security", "Sec"],
  ["reliability", "Reliability", "Rel"],
  ["compatibility", "Compatibility", "Compat"],
  ["testGap", "Test gap", "Tests"],
];

function dimensionsFor(report) {
  return Array.isArray(report.dimensions)
    ? report.dimensions.map(({ key, label, short }) => [key, label, short])
    : DEFAULT_DIMENSIONS;
}

const app = document.getElementById("app");
const meta = document.getElementById("meta");
let showValues = false;
let lastState = null;
let lastKey = "";
let view = { search: "", dimension: "all", severity: "all", sort: "severity-desc", groupByFile: false };

// All untrusted text goes through text nodes, never innerHTML.
function h(tag, props = {}, ...children) {
  const el = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (value == null || value === false) continue;
    if (key === "class") el.className = value;
    else if (key === "style") {
      for (const [prop, v] of Object.entries(value)) el.style.setProperty(prop, v);
    } else if (key.startsWith("on")) el.addEventListener(key.slice(2), value);
    else el.setAttribute(key, value === true ? "" : String(value));
  }
  for (const child of children.flat()) {
    if (child == null || child === false) continue;
    el.append(child instanceof Node ? child : String(child));
  }
  return el;
}

const isNum = (value) => typeof value === "number" && Number.isFinite(value);
const fixed = (value, digits = 2) => (isNum(value) ? value.toFixed(digits) : "–");

function ago(iso) {
  const seconds = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (seconds < 60) return "just now";
  const units = [
    [86400, "d"],
    [3600, "h"],
    [60, "m"],
  ];
  for (const [size, unit] of units) {
    if (seconds >= size) return `${Math.floor(seconds / size)}${unit} ago`;
  }
  return "just now";
}

function splitPath(path) {
  const text = String(path ?? "");
  const cut = text.lastIndexOf("/") + 1;
  return [text.slice(0, cut), text.slice(cut)];
}

// Sequential single-hue ramp, theme-aware through CSS custom properties.
function fill(p) {
  const x = Math.min(1, Math.max(0, p));
  return x <= 0.5
    ? `color-mix(in oklab, var(--seq-mid) ${(x * 200).toFixed(1)}%, var(--seq-lo))`
    : `color-mix(in oklab, var(--seq-hi) ${((x - 0.5) * 200).toFixed(1)}%, var(--seq-mid))`;
}

function section(label, aside, ...content) {
  const expanded = label === "Review funnel" || label === "Findings";
  return h(
    "details",
    { class: "block", open: expanded },
    h(
      "summary",
      { class: "block-head" },
      h("h2", {}, label),
      h("span", { class: "block-aside" }, aside),
    ),
    ...content,
  );
}

function quiet(title, detail, command) {
  return h(
    "div",
    { class: "quiet" },
    h("p", { class: "quiet-title" }, title),
    detail && h("p", { class: "quiet-detail" }, detail),
    command && h("code", { class: "quiet-command" }, command),
  );
}

function summary(report) {
  const findings = report.findings;
  const blocking = findings.filter((f) => f.action === "request_changes").length;
  const tests = report.contextFiles ?? report.changedTestFiles ?? [];
  const testLabel = report.mode === "codebase" ? "test files" : "changed tests";
  const stats = [
    { value: report.screenedFiles, label: "files" },
    { value: tests.length, label: testLabel, title: tests.join("\n") || null },
    { value: report.followedSignals, label: "investigated", title: "potential concerns at or above " + THRESHOLD.toFixed(2) + " reviewed for evidence" },
    { value: findings.length, label: "findings", cls: "lead" },
    { value: blocking, label: "request changes", cls: blocking > 0 ? "alert" : "" },
  ];
  return h(
    "dl",
    { class: "stats" },
    stats.map((stat) =>
      h(
        "div",
        { class: `stat ${stat.cls ?? ""}`, title: stat.title },
        h("dt", {}, stat.label),
        h("dd", {}, isNum(stat.value) ? stat.value : "–"),
      ),
    ),
  );
}

function workflow(report) {
  const flow = report.workflow;
  if (!flow) return null;

  const categoryCount = dimensionsFor(report).length;
  const fileKind = report.mode === "codebase" ? "complete source files" : "changed source files";
  const steps = [
    {
      value: flow.screenedCells,
      label: "risk checks",
      detail: "file × category",
      title: "One screening probability for every file and concern category",
    },
    {
      value: flow.thresholdSignals,
      label: "flagged",
      detail: "at least " + THRESHOLD.toFixed(2),
      title: "Screening probabilities at or above the follow-up threshold",
    },
    {
      value: flow.followedSignals,
      label: "investigated",
      detail: "evidence review",
      title: "Highest-risk potential concerns selected for deeper evidence review",
    },
    {
      value: flow.locatedFindings,
      label: "supported",
      detail: "evidence found",
      title: "Concerns supported by a concrete source region and mechanism",
    },
    {
      value: flow.routedFindings,
      label: "assigned",
      detail: "owner suggested",
      title: "Higher-severity findings assigned to a reviewer specialty",
    },
  ];

  return section(
    "Review funnel",
    null,
    h(
      "p",
      { class: "section-note" },
      report.screenedFiles + " " + fileKind + " were checked across " + categoryCount + " concern categories. Screening is broad; only higher probabilities continue to evidence review.",
    ),
    h(
      "ol",
      { class: "flow" },
      steps.map((step, index) =>
        h(
          "li",
          { title: step.title },
          index > 0 && h("span", { class: "flow-arrow", "aria-hidden": "true" }, "→"),
          h(
            "span",
            { class: "flow-step" },
            h("strong", {}, isNum(step.value) ? step.value : "–"),
            h("span", {}, step.label),
            h("small", {}, step.detail),
          ),
        ),
      ),
    ),
  );
}

function profiles(report) {
  const list = report.profiles;
  if (!Array.isArray(list) || list.length === 0) return null;
  const categoryHeading = report.mode === "codebase" ? "Role" : "Change";

  const table = h(
    "table",
    { class: "profiles" },
    h(
      "thead",
      {},
      h(
        "tr",
        {},
        ["File", categoryHeading, "Priority"].map((label) => h("th", { scope: "col" }, label)),
      ),
    ),
    h(
      "tbody",
      {},
      list.map((profile) => {
        const [dir, base] = splitPath(profile.file);
        const category = profile.category ?? profile.changeType ?? "–";
        const categoryConfidence = profile.categoryConfidence ?? profile.changeTypeConfidence;
        return h(
          "tr",
          {
            title: "category confidence " + fixed(categoryConfidence) + " · priority confidence " + fixed(profile.reviewPriorityConfidence),
          },
          h(
            "td",
            { class: "profile-file" },
            h("code", { title: profile.file }, h("span", { class: "dir" }, dir), h("span", { class: "base" }, base)),
          ),
          h("td", { class: "profile-type" }, String(category)),
          h("td", { class: "profile-priority" }, severityMeter(profile.reviewPriority)),
        );
      }),
    ),
  );

  return section(
    "Files selected for closer review",
    h("span", { class: "count" }, list.length),
    h(
      "p",
      { class: "section-note" },
      "These files had the highest screening scores. The category summarizes the file or change; review priority runs from 0 (routine) to 3 (specialist attention).",
    ),
    h("div", { class: "profiles-wrap" }, table),
  );
}

function matrix(report) {
  const dimensions = dimensionsFor(report);
  const rows = [...report.matrix].sort((a, b) => maxP(b, dimensions) - maxP(a, dimensions));
  const screeningNote = "Each cell is the estimated probability, from 0 to 1, that a file has that kind of concern. Darker cells mean higher probability; cells at or above " + THRESHOLD.toFixed(2) + " are flagged for deeper review. Screening is triage, not a confirmed finding.";

  const toggle = h(
    "button",
    {
      class: "toggle",
      type: "button",
      "aria-pressed": String(showValues),
      title: "Show the exact probability in every cell",
      onclick: () => {
        showValues = !showValues;
        render(lastState);
      },
    },
    "0.00",
  );

  const legend = h(
    "div",
    { class: "legend" },
    h("span", { class: "legend-end" }, "0"),
    h(
      "span",
      { class: "legend-ramp", role: "img", "aria-label": `Probability scale, threshold ${THRESHOLD}` },
      h("i", { class: "legend-tick", style: { left: `${THRESHOLD * 100}%` } }),
    ),
    h("span", { class: "legend-end" }, "1"),
    toggle,
  );

  if (rows.length === 0) {
    return section(
      "Risk screening by file",
      null,
      h("p", { class: "section-note" }, screeningNote),
      quiet("No source files screened"),
    );
  }

  const table = h(
    "table",
    { class: `matrix${showValues ? " show-values" : ""}` },
    h(
      "thead",
      {},
      h(
        "tr",
        {},
        h("th", { scope: "col", class: "file-col" }, h("span", { class: "sr" }, "File")),
        dimensions.map(([key, label, short]) =>
          h(
            "th",
            { scope: "col", title: label },
            h("span", { class: "long" }, label),
            h("abbr", { class: "short", title: label }, short),
          ),
        ),
      ),
    ),
    h(
      "tbody",
      {},
      rows.map((row) => {
        const [dir, base] = splitPath(row.file);
        return h(
          "tr",
          {},
          h(
            "th",
            { scope: "row", class: "file", title: row.file },
            h("span", { class: "path" }, h("span", { class: "dir" }, dir), h("span", { class: "base" }, base)),
          ),
          dimensions.map(([key, label]) => {
            const p = row[key];
            if (!isNum(p)) return h("td", { class: "cell missing" }, h("span", { class: "v" }, "–"));
            const hot = p >= THRESHOLD;
            return h(
              "td",
              {
                class: `cell${hot ? " hot" : ""}${p >= 0.55 ? " deep" : ""}`,
                style: { "--fill": fill(p) },
                title: `${row.file}\n${label} ${fixed(p)}`,
              },
              h("span", { class: "v" }, fixed(p)),
            );
          }),
        );
      }),
    ),
  );

  return section(
    "Risk screening by file",
    legend,
    h("p", { class: "section-note" }, screeningNote),
    h("div", { class: "matrix-wrap" }, table),
  );
}

function maxP(row, dimensions) {
  return Math.max(0, ...dimensions.map(([key]) => (isNum(row[key]) ? row[key] : 0)));
}

function severityMeter(severity) {
  const segments = Array.from({ length: SEVERITY_MAX }, (_, i) => {
    const amount = isNum(severity) ? Math.min(1, Math.max(0, severity - i)) : 0;
    return h("i", { style: { "--amount": `${(amount * 100).toFixed(0)}%` } });
  });
  return h(
    "span",
    { class: "severity" },
    h("span", { class: "meter", "aria-hidden": "true" }, segments),
    h("span", { class: "num" }, fixed(severity, 1)),
  );
}

function findings(report) {
  const list = report.findings;
  const labels = Object.fromEntries(dimensionsFor(report).map(([key, label]) => [key, label]));
  const count = h("span", { class: "count" }, list.length);
  const findingsNote = "These concerns passed screening and were tied to a concrete source region and mechanism. Severity runs from 0 (no meaningful impact) to 3 (critical). Findings are review leads, not proof of a defect.";

  if (list.length === 0) {
    const followed = report.followedSignals;
    const detail =
      followed > 0
        ? followed + " potential " + (followed === 1 ? "concern was" : "concerns were") + " investigated; none had enough evidence to become a finding"
        : "No screening probability reached the " + THRESHOLD.toFixed(2) + " follow-up threshold";
    return section(
      "Findings",
      count,
      h("p", { class: "section-note" }, findingsNote),
      quiet("No supported findings", detail),
    );
  }

  const body = h("div", { class: "findings-body" });

  const searchInput = h("input", {
    type: "search",
    class: "tool-search",
    placeholder: "Search findings",
    "aria-label": "Search findings",
    value: view.search,
    oninput: (event) => {
      view.search = event.target.value;
      refresh();
    },
  });

  const dimensionSelect = h(
    "select",
    {
      class: "tool-select",
      "aria-label": "Filter by concern",
      onchange: (event) => {
        view.dimension = event.target.value;
        refresh();
      },
    },
    h("option", { value: "all", selected: view.dimension === "all" }, "All concerns"),
    dimensionsFor(report).map(([key, label]) =>
      h("option", { value: key, selected: view.dimension === key }, label),
    ),
  );

  const severitySelect = h(
    "select",
    {
      class: "tool-select",
      "aria-label": "Filter by severity",
      onchange: (event) => {
        view.severity = event.target.value;
        refresh();
      },
    },
    h("option", { value: "all", selected: view.severity === "all" }, "Any severity"),
    h("option", { value: "routed", selected: view.severity === "routed" }, "Severity ≥ 1.5"),
    h("option", { value: "blocking", selected: view.severity === "blocking" }, "Blocking (request changes)"),
  );

  const sortSelect = h(
    "select",
    {
      class: "tool-select",
      "aria-label": "Sort findings",
      onchange: (event) => {
        view.sort = event.target.value;
        refresh();
      },
    },
    h("option", { value: "severity-desc", selected: view.sort === "severity-desc" }, "Severity ↓"),
    h("option", { value: "severity-asc", selected: view.sort === "severity-asc" }, "Severity ↑"),
    h("option", { value: "file", selected: view.sort === "file" }, "File"),
    h("option", { value: "dimension", selected: view.sort === "dimension" }, "Dimension"),
  );

  const groupToggle = h(
    "label",
    { class: "group-toggle" },
    h("input", {
      type: "checkbox",
      checked: view.groupByFile,
      onchange: (event) => {
        view.groupByFile = event.target.checked;
        refresh();
      },
    }),
    "Group by file",
  );

  const toolbar = h(
    "div",
    { class: "findings-toolbar" },
    searchInput,
    dimensionSelect,
    severitySelect,
    sortSelect,
    groupToggle,
  );

  function refresh() {
    const visible = filterFindings(report, view);
    count.textContent = String(visible.length);
    if (document.activeElement !== searchInput && searchInput.value !== view.search) {
      searchInput.value = view.search;
    }
    body.replaceChildren(
      visible.length === 0
        ? quiet("No findings match", "Try clearing the search or widening the filters.")
        : view.groupByFile
          ? findingsByFile(visible, labels, setSearch)
          : findingsTable(visible, labels, setSearch),
    );
  }

  function setSearch(term) {
    view.search = term;
    refresh();
  }

  refresh();

  return section(
    "Findings",
    count,
    h("p", { class: "section-note" }, findingsNote),
    toolbar,
    body,
  );
}

function filterFindings(report, view) {
  const q = view.search.trim().toLowerCase();
  let out = report.findings.slice();

  if (q) {
    out = out.filter((finding) =>
      [finding.file, finding.title, finding.mechanism, finding.why, finding.fix, finding.test].some(
        (value) => value != null && String(value).toLowerCase().includes(q),
      ),
    );
  }

  if (view.dimension !== "all") {
    out = out.filter((finding) => finding.dimension === view.dimension);
  }

  if (view.severity === "routed") {
    out = out.filter((finding) => isNum(finding.severity) && finding.severity >= 1.5);
  } else if (view.severity === "blocking") {
    out = out.filter((finding) => finding.action === "request_changes");
  }

  const severity = (finding) => (isNum(finding.severity) ? finding.severity : -1);
  switch (view.sort) {
    case "severity-asc":
      out.sort((a, b) => severity(a) - severity(b));
      break;
    case "file":
      out.sort((a, b) => String(a.file ?? "").localeCompare(String(b.file ?? "")));
      break;
    case "dimension":
      out.sort(
        (a, b) =>
          String(a.dimension ?? "").localeCompare(String(b.dimension ?? "")) ||
          severity(b) - severity(a),
      );
      break;
    default:
      out.sort((a, b) => severity(b) - severity(a));
  }

  return out;
}

function formatPrComment(finding, label = null) {
  const dimension = label ?? String(finding.dimension ?? "concern");
  const heading = String(finding.title ?? dimension);
  const lines = [`**${heading}** (${dimension}, severity ${fixed(finding.severity, 1)})`];
  if (finding.file != null) {
    lines.push(`\`${finding.file}${finding.line != null ? ":" + finding.line : ""}\``);
  }
  if (finding.why) lines.push(`> ${finding.why}`);
  if (finding.fix) lines.push(`- Fix: ${finding.fix}`);
  if (finding.test) lines.push(`- Test: ${finding.test}`);
  return lines.join("\n");
}

function copyButton(finding, label) {
  const text = formatPrComment(finding, label);
  let timer = null;
  const button = h(
    "button",
    {
      type: "button",
      class: "copy-btn",
      title: "Copy as PR comment",
      onclick: () => {
        const write = navigator.clipboard?.writeText
          ? navigator.clipboard.writeText(text)
          : Promise.reject(new Error("clipboard unavailable"));
        write.then(
          () => {
            button.textContent = "Copied";
            button.classList.add("copied");
          },
          () => {
            button.textContent = "Copy failed";
            button.classList.remove("copied");
          },
        );
        window.clearTimeout(timer);
        timer = window.setTimeout(() => {
          button.textContent = "Copy";
          button.classList.remove("copied");
        }, 1500);
      },
    },
    "Copy",
  );
  return button;
}

function findingRows(finding, labels, onFileClick) {
  const [dir, base] = splitPath(finding.file);
  const blocking = finding.action === "request_changes";
  const rows = [
    h(
      "tr",
      {
        title: `location confidence ${fixed(finding.locationConfidence)} · severity confidence ${fixed(finding.severityConfidence)}`,
      },
      h(
        "td",
        { class: "loc" },
        h(
          "button",
          {
            type: "button",
            class: "loc-link",
            title: "Search for this file",
            onclick: () => onFileClick(base),
          },
          h(
            "code",
            { title: `${finding.file}:${finding.line ?? "?"}` },
            h("span", { class: "dir" }, dir),
            h("span", { class: "base" }, base),
            h("span", { class: "line" }, `:${finding.line ?? "?"}`),
          ),
        ),
      ),
      h(
        "td",
        { class: "dim" },
        h("span", {}, labels[finding.dimension] ?? String(finding.dimension)),
        (finding.title || finding.mechanism) &&
          h("small", {}, String(finding.title || finding.mechanism)),
      ),
      h("td", { class: "sev" }, h("span", { class: "sr" }, "severity "), severityMeter(finding.severity)),
      h("td", { class: "owner" }, finding.owner ? String(finding.owner) : "–"),
      h(
        "td",
        { class: `act ${blocking ? "blocking" : "comment"}` },
        h("span", { class: "glyph", "aria-hidden": "true" }),
        blocking ? "Request changes" : finding.action === "comment" ? "Comment" : String(finding.action),
        copyButton(finding, labels[finding.dimension]),
      ),
    ),
  ];

  if (finding.evidence) {
    rows.push(
      h(
        "tr",
        { class: "evidence-row" },
        h(
          "td",
          { colspan: "5" },
          h(
            "details",
            {},
            h("summary", {}, "Evidence"),
            h("pre", {}, h("code", {}, String(finding.evidence))),
          ),
        ),
      ),
    );
  }

  if (finding.why || finding.fix || finding.test) {
    rows.push(
      h(
        "tr",
        { class: "suggestion-row" },
        h(
          "td",
          { colspan: "5" },
          h(
            "details",
            {},
            h("summary", {}, "Why & how to fix"),
            finding.why && h("p", {}, String(finding.why)),
            finding.fix && h("p", {}, h("strong", {}, "Fix: "), String(finding.fix)),
            finding.test && h("p", {}, h("strong", {}, "Test: "), String(finding.test)),
          ),
        ),
      ),
    );
  }

  return rows;
}

function findingsTable(items, labels, onFileClick) {
  return h(
    "table",
    { class: "findings" },
    h(
      "thead",
      {},
      h("tr", {}, ["Location", "Concern", "Severity", "Owner", "Action"].map((label) => h("th", { scope: "col" }, label))),
    ),
    h("tbody", {}, items.flatMap((finding) => findingRows(finding, labels, onFileClick))),
  );
}

function findingsByFile(items, labels, onFileClick) {
  const groups = [];
  const index = new Map();
  for (const finding of items) {
    const file = finding.file ?? "?";
    let group = index.get(file);
    if (!group) {
      group = { file, findings: [] };
      index.set(file, group);
      groups.push(group);
    }
    group.findings.push(finding);
  }
  return h(
    "div",
    { class: "findings-groups" },
    groups.map((group) => {
      const [dir, base] = splitPath(group.file);
      return h(
        "div",
        { class: "finding-group" },
        h(
          "h3",
          { class: "finding-group-head" },
          h(
            "code",
            { class: "finding-group-file", title: group.file },
            h("span", { class: "dir" }, dir),
            h("span", { class: "base" }, base),
          ),
          h("span", { class: "count" }, group.findings.length),
        ),
        findingsTable(group.findings, labels, onFileClick),
      );
    }),
  );
}

async function history() {
  const root = document.getElementById("history");
  if (!root) return;

  let data;
  try {
    const res = await fetch("/api/history", { cache: "no-store" });
    data = res.ok ? await res.json() : null;
  } catch {
    data = null;
  }

  if (data == null) {
    root.replaceChildren(section("History", null, quiet("History unavailable", "Could not load saved review history.")));
    return;
  }

  const entries = Array.isArray(data.entries) ? data.entries : [];
  const hotspots = Array.isArray(data.hotspots) ? data.hotspots : [];

  if (entries.length === 0) {
    root.replaceChildren(
      section(
        "History",
        h("span", { class: "count" }, "0"),
        quiet("No review history yet", "Saved scans will appear here as they complete."),
      ),
    );
    return;
  }

  root.replaceChildren(
    section(
      "History",
      h("span", { class: "count" }, entries.length),
      h(
        "p",
        { class: "section-note" },
        "Risk across saved reviews, oldest first, with the files that have drawn the most findings over time.",
      ),
      historyRisks(entries),
      historyHotspots(hotspots),
    ),
  );
}

function historyRisks(entries) {
  return [
    h("h3", { class: "history-subhead" }, "Risk over time"),
    h(
      "ol",
      { class: "history-list" },
      entries.map((entry) =>
        h(
          "li",
          { class: "history-row" },
          h("code", { class: "history-sha", title: entry.sha }, String(entry.sha ?? "").slice(0, 7) || "–"),
          h(
            "time",
            {
              class: "history-when",
              datetime: entry.savedAt,
              title: entry.savedAt ? new Date(entry.savedAt).toLocaleString() : null,
            },
            ago(entry.savedAt),
          ),
          severityMeter(entry.maxSeverity),
          h(
            "span",
            { class: "history-counts" },
            h("span", { class: "badge" }, `${entry.findings} finding${entry.findings === 1 ? "" : "s"}`),
            entry.blocking > 0 && h("span", { class: "badge badge-block" }, `${entry.blocking} blocking`),
          ),
        ),
      ),
    ),
  ];
}

function historyHotspots(hotspots) {
  if (hotspots.length === 0) return null;
  return [
    h("h3", { class: "history-subhead" }, "Hotspots & fix latency"),
    h(
      "div",
      { class: "history-wrap" },
      h(
        "table",
        { class: "hotspots" },
        h("thead", {}, h("tr", {}, ["File", "Risk", "Findings", "State"].map((label) => h("th", { scope: "col" }, label)))),
        h(
          "tbody",
          {},
          hotspots.map((hotspot) => {
            const [dir, base] = splitPath(hotspot.file);
            const open = hotspot.latestFindings > 0;
            return h(
              "tr",
              {},
              h(
                "td",
                { class: "loc" },
                h(
                  "code",
                  { title: hotspot.file },
                  h("span", { class: "dir" }, dir),
                  h("span", { class: "base" }, base),
                ),
              ),
              h("td", { class: "sev" }, severityMeter(hotspot.maxSeverity)),
              h("td", { class: "prob" }, String(hotspot.findings)),
              h(
                "td",
                {},
                open
                  ? h("span", { class: "badge badge-open" }, "open")
                  : h("span", { class: "badge badge-resolved" }, "resolved"),
              ),
            );
          }),
        ),
      ),
    ),
  ];
}

function renderMeta(state) {
  meta.replaceChildren();
  if (state?.status !== "ok") return;
  const scope = state.report.scope;
  const name = scope.split("/").filter(Boolean).pop() ?? scope;
  const mode = state.report.mode === "codebase" ? "Codebase scan" : "Change review";
  meta.append(
    h("span", { class: "mode", title: mode }, mode),
    h("span", { class: "sep", "aria-hidden": "true" }, "·"),
    h("span", { class: "scope", title: scope }, name),
    h("span", { class: "sep", "aria-hidden": "true" }, "·"),
    h("time", { datetime: state.savedAt, title: new Date(state.savedAt).toLocaleString() }, ago(state.savedAt)),
  );
}

function render(state) {
  lastState = state;
  renderMeta(state);
  document.body.dataset.status = state?.status ?? "offline";

  switch (state?.status) {
    case "ok":
      app.replaceChildren(
        ...[
          summary(state.report),
          workflow(state.report),
          profiles(state.report),
          matrix(state.report),
          findings(state.report),
          h("div", { id: "history" }),
        ].filter(Boolean),
      );
      history();
      break;
    case "empty":
      app.replaceChildren(quiet("No review yet", null, "momus review <path>"));
      break;
    case "error":
      app.replaceChildren(quiet("Unreadable report", `${state.message} · ${state.source}`));
      break;
    default:
      app.replaceChildren(quiet("Server unavailable", null, "momus dashboard"));
  }
}

async function load() {
  let state;
  try {
    const res = await fetch("/api/review", { cache: "no-store" });
    state = res.ok ? await res.json() : { status: "offline" };
  } catch {
    state = { status: "offline" };
  }
  const key = JSON.stringify(state);
  if (key === lastKey) return renderMeta(state);
  lastKey = key;
  render(state);
}

document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible") load();
});
window.addEventListener("focus", load);
load();
