#!/usr/bin/env python3
"""Deterministically author the v1 corpus. Labels stay outside review workspaces."""

from __future__ import annotations

import difflib
import hashlib
import json
from pathlib import Path
import textwrap

ROOT = Path(__file__).resolve().parent


def content(value: str) -> str:
    return textwrap.dedent(value).lstrip("\n")


def tree_hash(path: Path) -> str:
    entries = [{"path": file.relative_to(path).as_posix(),
                "sha256": hashlib.sha256(file.read_bytes()).hexdigest()}
               for file in sorted(path.rglob("*")) if file.is_file()]
    encoded = json.dumps(entries, sort_keys=True, separators=(",", ":"),
                         ensure_ascii=False).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def write_tree(path: Path, files: dict[str, str]) -> None:
    path.mkdir(parents=True, exist_ok=True)
    # Prevent removed files from entering a regenerated content identity.
    for old in path.rglob("*"):
        if old.is_file() and old.relative_to(path).as_posix() not in files:
            old.unlink()
    for name, value in files.items():
        destination = path / name
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_text(content(value), encoding="utf-8", newline="\n")


def make_diff(base: dict[str, str], head: dict[str, str]) -> str:
    output = []
    for name in sorted(base.keys() | head.keys()):
        before, after = content(base.get(name, "")), content(head.get(name, ""))
        if before != after:
            output.extend(difflib.unified_diff(before.splitlines(True), after.splitlines(True),
                                              fromfile=f"a/{name}" if name in base else "/dev/null",
                                              tofile=f"b/{name}" if name in head else "/dev/null"))
    return "".join(output)


def issue(identifier: str, file: str, anchor: str, head: dict[str, str], dimension: str,
          trigger: str, contract: str, consequence: str, acceptable: str, fix: str) -> dict:
    lines = content(head[file]).splitlines()
    matches = [index + 1 for index, line in enumerate(lines) if anchor in line]
    if len(matches) != 1:
        raise ValueError(f"{identifier}: ambiguous location {anchor!r}")
    return {"id": identifier, "file": file, "startLine": matches[0], "endLine": matches[0],
            "dimension": dimension, "trigger": trigger, "contract": contract,
            "consequence": consequence, "acceptableFinding": acceptable,
            "explanation": {
                "sourceCorrectness": f"Ground the claim in {file}:{matches[0]} and the visible caller/tests and repository contract.",
                "specificity": f"Explain the concrete trigger: {trigger}",
                "actionability": fix,
                "uncertainty": "Limit the conclusion to this authored fixture and its explicit contract; do not infer production exposure or benchmark-wide performance."},
            "fix": fix}


CASES: list[dict] = []


def add_pair(identifier: str, language: str, dimension: str, description: str,
             base: dict[str, str], defect: dict[str, str], fixed: dict[str, str],
             file: str, anchor: str, trigger: str, contract: str,
             consequence: str, acceptable: str, fix: str, *, auxiliary: bool = False,
             native_dimension: str | None = None) -> None:
    write_tree(ROOT / "fixtures" / identifier / "base", base)
    for suffix, tree, expectation in [("defect", defect, "defect"), ("fixed", fixed, "clean")]:
        directory = ROOT / "fixtures" / identifier / suffix
        write_tree(directory, tree)
        diff_path = ROOT / "fixtures" / identifier / f"{suffix}.diff"
        diff_path.write_text(make_diff(base, tree), encoding="utf-8", newline="\n")
        case_id = f"{identifier}-{suffix}"
        CASES.append({
            "id": case_id, "pairId": identifier, "split": "holdout",
            "slice": "auxiliary" if auxiliary else "core", "language": language,
            "expectation": expectation, "dimension": dimension,
            "base": f"fixtures/{identifier}/base", "head": f"fixtures/{identifier}/{suffix}",
            "diff": f"fixtures/{identifier}/{suffix}.diff",
            "source": {"kind": "authored", "license": "MIT", "description": description,
                       "baseSha256": tree_hash(ROOT / "fixtures" / identifier / "base"),
                       "headSha256": tree_hash(directory),
                       "diffSha256": hashlib.sha256(diff_path.read_bytes()).hexdigest()},
            "issues": [issue(f"{identifier}-issue", file, anchor, defect,
                             native_dimension or dimension, trigger, contract, consequence,
                             acceptable, fix)] if expectation == "defect" else [],
            "abstention": None,
            "reproducer": ["python3", "reproduce.py", "--case", case_id],
            "annotation": {"status": "pending", "reviewer": None,
                           "rationale": "Authored source annotation awaiting a separate reviewer; behavioral reproduction is not independent annotation validation."}})


def rust_package(name: str, readme: str, module: str, source: str, caller: str,
                 tests: str = "") -> dict[str, str]:
    files = {"Cargo.toml": f'[package]\nname = "{name}"\nversion = "0.1.0"\nedition = "2021"\n',
             "README.md": readme, f"src/{module}.rs": source, "src/main.rs": caller}
    if tests:
        files[f"src/{module}.rs"] += content(tests)
    return files


def build() -> None:
    # Core Rust: integer arithmetic, authorization, capped backoff, wire compatibility.
    readme = "# Invoice service\nThe checkout caller uses cents. Multiply the total by the basis-point discount and round that discount amount down, then subtract it from the total. For example, 2001 cents at 2500 basis points yields a 500-cent discount and 1501 cents payable. Inputs are any u64 cents and 0..10000 basis points.\n"
    base = rust_package("invoice-service", readme, "invoice", "pub fn total(cents: u64) -> u64 { cents }\n", "mod invoice;\nfn main() { assert_eq!(invoice::total(2000), 2000); }\n")
    caller = "mod invoice;\nfn main() {\n    assert_eq!(invoice::discounted_total(2000, 2500), 1500);\n    assert_eq!(invoice::discounted_total(2001, 2500), 1501);\n    assert_eq!(invoice::discounted_total(u64::MAX, 10000), 0);\n    assert_eq!(invoice::discounted_total(u64::MAX, 0), u64::MAX);\n}\n"
    bad = "pub fn discounted_total(cents: u64, basis_points: u16) -> u64 {\n    assert!(basis_points <= 10000);\n    cents - cents * (u64::from(basis_points) / 10000)\n}\n"
    good = "pub fn discounted_total(cents: u64, basis_points: u16) -> u64 {\n    assert!(basis_points <= 10000);\n    cents - (u128::from(cents) * u128::from(basis_points) / 10000) as u64\n}\n"
    test = "\n#[cfg(test)] mod tests {\n    #[test] fn no_discount() { assert_eq!(super::discounted_total(2000, 0), 2000); }\n}\n"
    defect = rust_package("invoice-service", readme, "invoice", bad, caller, test)
    fixed = rust_package("invoice-service", readme, "invoice", good, caller, test)
    add_pair("rust-invoice", "rust", "correctness", "Original authored invoice discount change; no imported project or PR example.", base, defect, fixed,
             "src/invoice.rs", "cents - cents", "Checkout applies a 2500-basis-point discount to 2000 cents.",
             "Multiply before integer division, round the discount amount down and subtract it; the payable total is 1500 cents.",
             "The division truncates 2500/10000 to zero and charges 2000 cents, overcharging by 500.",
             "Identify integer-division order in discounted_total, the 2500/2000 trigger and the wrong checkout total.",
             "Multiply cents by basis points in u128, divide the discount amount by 10000, then subtract that rounded discount from cents.")

    readme = "# Workspace deletion API\nAuthenticated Actor.can_manage is the sole deletion authority. The request role is an untrusted display preference supplied by the HTTP caller.\n"
    base_source = "pub struct Actor { pub can_manage: bool }\npub fn may_delete(actor: &Actor) -> bool { actor.can_manage }\n"
    base = rust_package("workspace-api", readme, "auth", base_source, "mod auth;\nfn main() { assert!(!auth::may_delete(&auth::Actor { can_manage: false })); }\n")
    bad = "pub struct Actor { pub can_manage: bool }\npub fn may_delete(actor: &Actor, request_role: &str) -> bool {\n    actor.can_manage || request_role == \"admin\"\n}\n"
    good = "pub struct Actor { pub can_manage: bool }\npub fn may_delete(actor: &Actor, _request_role: &str) -> bool {\n    actor.can_manage\n}\n"
    caller = "mod auth;\nfn main() {\n    let viewer = auth::Actor { can_manage: false };\n    assert!(!auth::may_delete(&viewer, \"admin\"));\n}\n"
    test = "\n#[cfg(test)] mod tests {\n    #[test] fn manager_allowed() { assert!(super::may_delete(&super::Actor { can_manage: true }, \"member\")); }\n}\n"
    defect = rust_package("workspace-api", readme, "auth", bad, caller, test)
    fixed = rust_package("workspace-api", readme, "auth", good, caller, test)
    add_pair("rust-authority", "rust", "security", "Original authored authenticated-role boundary change.", base, defect, fixed,
             "src/auth.rs", "actor.can_manage ||", "An authenticated viewer passes request_role=admin when requesting workspace deletion.",
             "Only server-authenticated Actor.can_manage may authorize deletion; request_role is untrusted.",
             "The caller-controlled admin string grants deletion authority to a viewer.",
             "Trace the request role into the authorization OR branch and distinguish it from the authenticated permission.",
             "Ignore the untrusted display role for authorization and check only Actor.can_manage.")

    readme = "# Retry scheduler\nPersisted attempt counts may be any u32. delay_ms must return a delay at most 10000 milliseconds without panicking, including restored counters above 63.\n"
    base = rust_package("retry-scheduler", readme, "retry", "pub fn delay_ms(_attempt: u32) -> u64 { 100 }\n", "mod retry;\nfn main() { assert_eq!(retry::delay_ms(0), 100); }\n")
    bad = "pub fn delay_ms(attempt: u32) -> u64 {\n    (100 * (1u64 << attempt)).min(10000)\n}\n"
    good = "pub fn delay_ms(attempt: u32) -> u64 {\n    100u64.checked_mul(2u64.saturating_pow(attempt)).unwrap_or(10000).min(10000)\n}\n"
    caller = "mod retry;\nfn main() { assert_eq!(retry::delay_ms(64), 10000); }\n"
    test = "\n#[cfg(test)] mod tests {\n    #[test] fn initial_delay() { assert_eq!(super::delay_ms(0), 100); }\n}\n"
    defect = rust_package("retry-scheduler", readme, "retry", bad, caller, test)
    fixed = rust_package("retry-scheduler", readme, "retry", good, caller, test)
    add_pair("rust-backoff", "rust", "reliability", "Original authored retry backoff cap implementation.", base, defect, fixed,
             "src/retry.rs", "1u64 << attempt", "A restored retry job has attempt=64.",
             "All u32 persisted counters must yield a bounded delay without panic.",
             "The shift overflows before min applies; debug builds panic before the cap, aborting delay calculation instead of returning a bounded delay.",
             "Identify the shift/overflow occurring before the cap and connect it to restored counters and aborted delay calculation; do not infer job loss from absent persistence evidence.",
             "Use saturating exponentiation and checked multiplication, then cap the resulting delay.")

    readme = "# Worker health endpoint\nThe stable JSON response key is state; existing clients search for state=ready. Tracing may add a trace key while retaining state. Trace identifiers are generated internally using only lowercase ASCII letters, digits and hyphens.\n"
    base = rust_package("worker-health", readme, "wire", 'pub fn encode() -> String { "{\\\"state\\\":\\\"ready\\\"}".into() }\n', 'mod wire;\nfn main() { assert!(wire::encode().contains("\\\"state\\\":\\\"ready\\\"")); }\n')
    bad = 'pub fn encode(trace: &str) -> String {\n    format!("{{\\\"status\\\":\\\"ready\\\",\\\"trace\\\":\\\"{}\\\"}}", trace)\n}\n'
    good = bad.replace("status", "state")
    caller = 'mod wire;\nfn main() {\n    let response = wire::encode("health-123");\n    assert!(response.contains("\\\"state\\\":\\\"ready\\\""));\n    assert!(response.contains("health-123"));\n}\n'
    defect = rust_package("worker-health", readme, "wire", bad, caller)
    fixed = rust_package("worker-health", readme, "wire", good, caller)
    add_pair("rust-wire", "rust", "compatibility", "Original authored additive tracing feature and stable wire contract.", base, defect, fixed,
             "src/wire.rs", "status", "An existing health client parses the state key after the trace feature is deployed.",
             "The state response key remains stable while trace is additive.",
             "Renaming state to status makes an otherwise healthy worker unreadable to existing clients.",
             "Name the removed state key and cite the existing client, rather than asserting that any new JSON field is incompatible.",
             "Retain state and add trace without renaming the existing wire key.")

    # Core JavaScript: tenant boundary, queue drain, response schema, test-gap mutation control.
    readme = "# Tenant notes\nRows are scoped by tenant and id together. Updating another tenant's note must return false and leave that row unchanged.\n"
    base = {"README.md": readme, "package.json": '{"name":"tenant-notes","version":"1.0.0","private":true}\n',
            "notes.js": 'exports.get = (rows, tenant, id) => rows.find(row => row.tenant === tenant && row.id === id);\n',
            "tests.js": 'const assert = require("node:assert/strict");\nconst notes = require("./notes");\nassert.equal(notes.get([], "red", 9), undefined);\n'}
    bad = 'exports.updateTitle = (rows, tenant, id, title) => {\n  const row = rows.find(row => row.id === id);\n  if (!row) return false;\n  row.title = title;\n  return true;\n};\n'
    good = bad.replace("row.id === id", "row.tenant === tenant && row.id === id")
    tests = 'const assert = require("node:assert/strict");\nconst notes = require("./notes");\nconst rows = [{tenant:"red", id:1, title:"old"}];\nassert.equal(notes.updateTitle(rows, "red", 1, "new"), true);\nassert.equal(rows[0].title, "new");\n'
    defect = {**base, "notes.js": bad, "tests.js": tests}
    fixed = {**base, "notes.js": good, "tests.js": tests}
    add_pair("js-tenant", "javascript", "security", "Original authored tenant-scoped note update endpoint.", base, defect, fixed,
             "notes.js", "const row = rows.find", "Tenant red submits id=9 that exists only in tenant blue.",
             "Locate notes by both tenant and id before mutation.",
             "The id-only lookup mutates tenant blue's title using tenant red's request.",
             "Describe the id-only row lookup, the cross-tenant trigger and unauthorized mutation.",
             "Include row.tenant === tenant in the update lookup, as the existing get contract does.")

    readme = "# Dispatch queue\nThe dispatcher owns its mutable queue. drain must return every queued job once, in order, and leave the queue empty.\n"
    base = {"README.md": readme, "queue.js": 'exports.take = queue => queue.shift();\n',
            "dispatcher.js": 'const {take} = require("./queue");\nexports.dispatch = queue => take(queue);\n',
            "tests.js": 'const assert = require("node:assert/strict");\nassert.equal(require("./queue").take(["a"]), "a");\n'}
    bad = 'exports.drain = queue => {\n  const jobs = [];\n  for (let index = 0; index < queue.length; index++) jobs.push(queue.shift());\n  return jobs;\n};\n'
    good = bad.replace("for (let index = 0; index < queue.length; index++)", "while (queue.length)")
    caller = 'const {drain} = require("./queue");\nexports.dispatch = queue => drain(queue);\n'
    tests = 'const assert = require("node:assert/strict");\nassert.deepEqual(require("./dispatcher").dispatch(["a"]), ["a"]);\n'
    defect = {**base, "queue.js": bad, "dispatcher.js": caller, "tests.js": tests}
    fixed = {**defect, "queue.js": good}
    add_pair("js-drain", "javascript", "reliability", "Original authored batch dispatch addition with mutable queue ownership.", base, defect, fixed,
             "queue.js", "for (let index", "Dispatcher receives three queued jobs a, b and c.",
             "Drain all queued jobs exactly once and leave the owned queue empty.",
             "Shifting shrinks queue.length while the loop index grows; c remains queued and is not dispatched.",
             "Explain the index/length interaction and show that a three-job batch dispatches only two jobs.",
             "Loop while the queue is nonempty or snapshot the original length before removing jobs.")

    readme = "# Leased transport worker\nA transport lease must remain active until sendAsync resolves or rejects. The worker owns the lease and releases it after every completed operation, including error outcomes. The async send yields before checking that the lease is still active.\n"
    base = {"README.md": readme,
            "lease.js": 'class Lease {\n  constructor() { this.released = false; }\n  sendSync(message) {\n    if (this.released) throw new Error("inactive lease");\n    return `sent:${message}`;\n  }\n  release() { this.released = true; }\n}\nmodule.exports = {Lease};\n',
            "forward.js": 'exports.forward = (lease, message) => {\n  try {\n    return lease.sendSync(message);\n  } finally {\n    lease.release();\n  }\n};\n',
            "worker.js": 'const {Lease} = require("./lease");\nconst {forward} = require("./forward");\nexports.handle = message => forward(new Lease(), message);\n',
            "tests.js": 'const assert = require("node:assert/strict");\nassert.equal(require("./worker").handle("ping"), "sent:ping");\n'}
    lease_async = base["lease.js"].replace('  release() {', '  async sendAsync(message) {\n    await Promise.resolve();\n    if (this.released) throw new Error("lease released before send completed");\n    if (message === "fail") throw new Error("transport rejected send");\n    return `sent:${message}`;\n  }\n  release() {')
    bad = 'exports.forward = async (lease, message) => {\n  try {\n    return lease.sendAsync(message);\n  } finally {\n    lease.release();\n  }\n};\n'
    good = bad.replace("return lease.sendAsync", "return await lease.sendAsync")
    tests = 'const assert = require("node:assert/strict");\nconst {forward} = require("./forward");\nconst lease = {sendAsync: async message => `sent:${message}`, release() {}};\nforward(lease, "ping").then(value => assert.equal(value, "sent:ping"));\n'
    defect = {**base, "lease.js": lease_async, "forward.js": bad,
              "worker.js": base["worker.js"].replace("exports.handle = message", "exports.handle = async message"),
              "tests.js": tests}
    fixed = {**defect, "forward.js": good}
    add_pair("js-async-lease", "javascript", "reliability", "Original authored synchronous-to-asynchronous leased transport change with observable completion and rejection behavior.", base, defect, fixed,
             "forward.js", "return lease.sendAsync", "worker.handle('ping') awaits a send that yields before reading the active lease.",
             "The worker must retain the lease until the send promise settles and release it after completion or rejection.",
             "Returning the promise directly runs finally immediately, releasing the lease before sendAsync resumes; the otherwise valid send rejects.",
             "Explain async promise adoption versus the timing of finally and the concrete released-lease error after the microtask yield.",
             "Await lease.sendAsync inside try so finally releases the lease only after success or rejection.")

    readme = "# Catalog pagination\nThe stable page shape is {items, nextCursor}. Existing collector callers continue until nextCursor is null. The new hasMore field is additive.\n"
    base = {"README.md": readme, "catalog.js": 'exports.page = (rows, cursor) => ({items: rows.slice(cursor, cursor + 2), nextCursor: cursor + 2 < rows.length ? cursor + 2 : null});\n',
            "collector.js": 'const catalog = require("./catalog");\nexports.firstCursor = rows => catalog.page(rows, 0).nextCursor;\n'}
    bad = 'exports.page = (rows, cursor) => {\n  const next = cursor + 2 < rows.length ? cursor + 2 : null;\n  return {items: rows.slice(cursor, cursor + 2), next, hasMore: next !== null};\n};\n'
    good = bad.replace("), next, hasMore", "), nextCursor: next, hasMore")
    defect = {**base, "catalog.js": bad}
    fixed = {**base, "catalog.js": good}
    add_pair("js-pagination", "javascript", "compatibility", "Original authored catalog pagination metadata feature.", base, defect, fixed,
             "catalog.js", "return {items", "The unchanged collector requests the first page of a four-row catalog.",
             "Existing callers read nextCursor; hasMore must be added without removing nextCursor.",
             "The renamed next property makes collector.firstCursor return undefined instead of 2.",
             "Tie the removed nextCursor return property to the unchanged collector and truncated traversal.",
             "Return nextCursor: next alongside the additive hasMore field.")

    readme = "# Refund ledger\nRefund requests carry an idempotency key. The first key credits balance and returns true; repeating it returns false without a second credit. Tests for this new public operation must exercise the repeat-key branch.\n"
    base = {"README.md": readme, "ledger.js": 'exports.charge = (state, cents) => { state.balance -= cents; };\n',
            "tests.js": 'const assert = require("node:assert/strict");\nconst ledger = require("./ledger");\nconst state = {balance:100};\nledger.charge(state, 20);\nassert.equal(state.balance, 80);\n'}
    ledger = 'exports.refund = (state, key, cents) => {\n  if (state.refunds.has(key)) return false;\n  state.refunds.add(key);\n  state.balance += cents;\n  return true;\n};\n'
    tests = 'const assert = require("node:assert/strict");\nconst ledger = require("./ledger");\nconst state = {balance:0, refunds:new Set()};\nassert.equal(ledger.refund(state, "order-7", 30), true);\nassert.equal(state.balance, 30);\n'
    defect = {**base, "ledger.js": ledger, "tests.js": tests}
    fixed = {**defect, "tests.js": tests + 'assert.equal(ledger.refund(state, "order-7", 30), false);\nassert.equal(state.balance, 30);\n'}
    add_pair("js-refund-tests", "javascript", "testGap", "Original authored regression-test omission for an otherwise correct idempotent refund implementation.", base, defect, fixed,
             "tests.js", 'ledger.refund(state, "order-7", 30)', "A repeat-key guard is removed in a future regression while the new test suite runs.",
             "The public refund idempotency contract needs a test asserting repeated keys do not credit twice.",
             "The suite observes only the first refund; a mutation deleting the guard still passes while duplicate credits occur.",
             "Identify the missing repeat-key test and the specific surviving guard-removal mutation; do not claim the current production guard is broken.",
             "Call refund twice with the same key and assert the second result is false and balance remains unchanged.")

    # Core Python: exact cents, export containment, legacy config, negative signature tests.
    readme = "# Price import\nUpstream validation limits price strings to nonnegative decimal amounts at most 9999999.99, with at most two fractional digits. parse_cents must preserve their exact cent value; 0.29 is 29 cents.\n"
    base = {"README.md": readme, "prices.py": 'def parse_cents(value):\n    return int(value) * 100\n',
            "importer.py": 'from prices import parse_cents\n\ndef import_price(row):\n    return {"cents": parse_cents(row["price"])}\n',
            "tests.py": 'from prices import parse_cents\nassert parse_cents("2") == 200\n'}
    bad = 'def parse_cents(value):\n    return int(float(value) * 100)\n'
    good = 'from decimal import Decimal\n\ndef parse_cents(value):\n    return int(Decimal(value) * 100)\n'
    defect = {**base, "prices.py": bad, "tests.py": 'from prices import parse_cents\nassert parse_cents("2.50") == 250\n'}
    fixed = {**defect, "prices.py": good}
    add_pair("python-cents", "python", "correctness", "Original authored decimal-price import feature.", base, defect, fixed,
             "prices.py", "return int(float", "The importer receives price='0.29'.",
             "Decimal input must retain its exact cents without binary floating-point truncation.",
             "float('0.29') * 100 is below 29 and int truncates the imported price to 28 cents.",
             "Show the 0.29 trigger and binary-float-to-int undercount in parse_cents, rather than a general numeric-style complaint.",
             "Parse with Decimal and scale by 100 before converting an input already constrained to cent precision.")

    readme = "# Export download\nHTTP names may reference nested export folders. resolve_export must reject names resolving outside the configured export root, including ../ traversal; valid nested names remain allowed.\n"
    base = {"README.md": readme, "exports.py": 'def resolve_export(root, name):\n    if "/" in name:\n        raise ValueError("flat export names only")\n    return root / name\n',
            "handler.py": 'from exports import resolve_export\n\ndef download_path(root, query):\n    return resolve_export(root, query["name"])\n',
            "tests.py": 'from pathlib import Path\nfrom exports import resolve_export\nassert resolve_export(Path("/exports"), "report.csv") == Path("/exports/report.csv")\n'}
    bad = 'def resolve_export(root, name):\n    return (root / name).resolve()\n'
    good = 'def resolve_export(root, name):\n    root = root.resolve()\n    target = (root / name).resolve()\n    target.relative_to(root)\n    return target\n'
    tests = 'from pathlib import Path\nfrom exports import resolve_export\nassert resolve_export(Path("/exports"), "month/report.csv") == Path("/exports/month/report.csv")\n'
    defect = {**base, "exports.py": bad, "tests.py": tests}
    fixed = {**defect, "exports.py": good}
    add_pair("python-export", "python", "security", "Original authored nested-export path handling change.", base, defect, fixed,
             "exports.py", "return (root / name).resolve", "The HTTP query name is '../private.csv'.",
             "Every resolved download path must remain under the configured export root.",
             "resolve normalizes traversal but does not confine it; the handler selects a file outside the export directory.",
             "Trace untrusted query name through download_path and distinguish normalization from containment validation.",
             "Resolve both paths, require target.relative_to(root) to succeed, and reject ValueError before opening the file.")

    readme = "# Worker configuration\nExisting config files use timeout in seconds. New timeout_ms values take precedence when present; legacy timeout remains supported. Scheduler callers expect a seconds value.\n"
    base = {"README.md": readme, "config.py": 'def timeout_seconds(config):\n    return config.get("timeout", 10)\n',
            "scheduler.py": 'from config import timeout_seconds\n\ndef job_timeout(config):\n    return timeout_seconds(config)\n',
            "examples/legacy.json": '{"timeout":2}\n',
            "tests.py": 'from config import timeout_seconds\nassert timeout_seconds({"timeout":2}) == 2\n'}
    bad = 'def timeout_seconds(config):\n    return config.get("timeout_ms", 10000) / 1000\n'
    good = 'def timeout_seconds(config):\n    if "timeout_ms" in config:\n        return config["timeout_ms"] / 1000\n    return config.get("timeout", 10)\n'
    tests = 'from config import timeout_seconds\nassert timeout_seconds({"timeout_ms":2500}) == 2.5\n'
    defect = {**base, "config.py": bad, "tests.py": tests}
    fixed = {**defect, "config.py": good}
    add_pair("python-config", "python", "compatibility", "Original authored millisecond configuration addition retaining a legacy config sample.", base, defect, fixed,
             "config.py", 'config.get("timeout_ms"', "The unchanged legacy config example {'timeout':2} is loaded by the scheduler.",
             "Legacy timeout seconds remain honored unless timeout_ms is explicitly supplied.",
             "The parser ignores timeout and silently changes the legacy job limit from 2 to 10 seconds.",
             "Name the old timeout key, the existing sample/caller and the fallback value causing the regression.",
             "Use timeout_ms only when present, otherwise preserve the timeout-seconds lookup.")

    readme = "# Signed webhook\nSignatures are HMAC-SHA256 hexadecimal strings. valid_webhook must accept the signed payload and reject a modified payload or incorrect signature. New verifier tests must cover rejection as well as acceptance.\n"
    signature = 'import hashlib\nimport hmac\n\ndef signature(payload, secret):\n    return hmac.new(secret, payload, hashlib.sha256).hexdigest()\n'
    base = {"README.md": readme, "webhook.py": signature,
            "tests.py": 'from webhook import signature\nassert len(signature(b"event", b"secret")) == 64\n'}
    verifier = signature + '\ndef valid_webhook(payload, supplied, secret):\n    expected = signature(payload, secret)\n    return hmac.compare_digest(expected, supplied)\n'
    tests = 'from webhook import signature, valid_webhook\npayload, secret = b"event", b"secret"\nsigned = signature(payload, secret)\nassert valid_webhook(payload, signed, secret)\n'
    defect = {**base, "webhook.py": verifier, "tests.py": tests}
    fixed = {**defect, "tests.py": tests + 'assert not valid_webhook(b"tampered", signed, secret)\nassert not valid_webhook(payload, "0" * 64, secret)\n'}
    add_pair("python-signature-tests", "python", "testGap", "Original authored negative-test omission for a correct HMAC verifier.", base, defect, fixed,
             "tests.py", "assert valid_webhook(payload", "The verifier comparison is accidentally replaced by unconditional True in a later change.",
             "Tests must assert tampered payloads and invalid signatures are rejected.",
             "The acceptance-only test passes the always-accept mutant, allowing an authentication bypass regression to escape.",
             "Point to the absent rejection assertions and the unconditional-accept mutant; do not report a bypass in the current correct implementation.",
             "Add tampered-payload and mismatched-signature rejection assertions while retaining the valid-signature case.")

    # Supported auxiliary dimensions, deliberately scored separately from core model claims.
    docs_readme = '# Client quickstart\nSee [the client API](src/client.rs). connect accepts an endpoint; the example must compile against the current public API. The caller imports connect from the linked API before using this excerpt.\n\n```rust\n// connect is imported from the linked client API by the caller.\nfn main() {\n    let _connection = connect("db");\n}\n```\n'
    base = {"README.md": docs_readme, "src/client.rs": 'pub fn connect(endpoint: &str) -> String { endpoint.into() }\n',
            "tests.rs": 'mod client;\nfn main() { assert_eq!(client::connect("db"), "db"); }\n'}
    new_source = 'pub fn connect(endpoint: &str, retries: u8) -> String {\n    format!("{}:{}", endpoint, retries)\n}\n'
    defect = {**base, "src/client.rs": new_source,
              "tests.rs": 'mod client;\nfn main() { assert_eq!(client::connect("db", 3), "db:3"); }\n'}
    fixed = {**defect, "README.md": docs_readme.replace('connect("db")', 'connect("db", 3)').replace("accepts an endpoint;", "accepts an endpoint and retry count;")}
    add_pair("docs-client-arity", "docs", "docs", "Original authored Rust API/documentation fixed-arity mismatch, within the supported docs checker.", base, defect, fixed,
             "README.md", 'connect("db")', "A reader compiles the unchanged source-linked quickstart excerpt with connect imported after it gains the retries parameter.",
             "The source-linked Rust quickstart must call the current fixed-arity public function.",
             "The example supplies one argument to a two-argument function and fails to compile.",
             "Tie README's source link and connect call to src/client.rs's changed signature; report a concrete arity mismatch.",
             "Pass the retry count in the quickstart and describe the updated public arguments.", auxiliary=True, native_dimension="correctness")

    readme = "# Record writer\ncodec-fixture is an authored local stand-in, loaded from vendor rather than a registry. Package metadata pins its interface version. v2 removes encode and replaces it with stringify; application callers must migrate together.\n"
    package_v1 = '{\n  "name": "record-writer",\n  "version": "1.0.0",\n  "dependencies": {"codec-fixture": "1.2.0"}\n}\n'
    package_v2 = package_v1.replace("1.2.0", "2.0.0")
    base = {"README.md": readme, "package.json": package_v1,
            "app.js": 'const codec = require("./vendor/codec-fixture");\nexports.writeRecord = record => codec.encode(record);\n',
            "vendor/codec-fixture/index.js": 'exports.encode = record => JSON.stringify(record);\n',
            "vendor/codec-fixture/CHANGELOG.md": '# codec-fixture 1.2.0\nInitial encode(record) API.\n',
            "tests.js": 'const assert = require("node:assert/strict");\nassert.equal(require("./app").writeRecord({id:7}), \'{"id":7}\');\n'}
    defect = {**base, "package.json": package_v2,
              "vendor/codec-fixture/index.js": 'exports.stringify = record => JSON.stringify(record);\n',
              "vendor/codec-fixture/CHANGELOG.md": '# codec-fixture 2.0.0\nBreaking: removed encode(record); migration required to stringify(record).\n\n# codec-fixture 1.2.0\nInitial encode(record) API.\n'}
    fixed = {**defect, "app.js": defect["app.js"].replace("codec.encode", "codec.stringify")}
    add_pair("dependency-codec-api", "dependency", "dependency", "Original authored local dependency stand-in with exact target release notes and visible downstream caller; no registry package claims.", base, defect, fixed,
             "vendor/codec-fixture/index.js", "exports.stringify", "The package pin and vendored interface upgrade to codec-fixture 2.0.0 while app.writeRecord still calls encode.",
             "The v2 release explicitly removes encode; every downstream caller must migrate to stringify.",
             "Writing a record raises TypeError because the installed v2 module has no encode function.",
             "Cite the changed export in vendor/codec-fixture/index.js:1, the remaining encode caller in app.js:2 and the exact version/target release notes. A generic major-version advisory alone does not establish this issue.",
             "Migrate app.writeRecord to codec.stringify with the existing output regression test.", auxiliary=True, native_dimension="compatibility")

    add_abstention("rust-macro-unknown", "rust", "security",
                   {"README.md": "# Policy plugin boundary\nThe private policy_plugin macro implementation and its generated authorization checks are outside this supplied snapshot.\n",
                    "src/lib.rs": 'policy_plugin::guarded! { pub fn delete_workspace(id: u64) { storage::delete(id); } }\n',
                    "Cargo.toml": '[package]\nname="plugin-host"\nversion="0.1.0"\n[dependencies]\npolicy_plugin = { path = "../private-policy-plugin" }\n'},
                   "src/lib.rs", 'policy_plugin::guarded! { pub fn delete_workspace(id: u64) { storage::delete_with_audit(id); } }\n',
                   ["The authorization-generating private macro implementation and storage caller contract are not supplied."],
                   "State that authorization cannot be determined from the macro invocation; request expansion/source before asserting a bypass.")
    add_abstention("js-transport-unknown", "javascript", "reliability",
                   {"README.md": "# External transport adapter\nOnly the caller is supplied. The external SDK's acknowledgment, durability and retry contract and implementation are unavailable.\n",
                    "publish.js": 'exports.publish = async (transport, event) => transport.publish(event, {ack:true});\n'},
                   "publish.js", 'exports.publish = async (transport, event) => transport.publish(event, {mode:"durable"});\n',
                   ["No transport SDK implementation, version or acknowledgment/durability semantics are visible."],
                   "Flag durability as unknown and request the SDK contract; do not infer data loss from option names alone.")
    add_abstention("python-dependency-unknown", "python", "compatibility",
                   {"README.md": "# External analyzer\nThe vendor release notes and resolved package source are unavailable. The visible caller uses analyze(text).\n",
                    "requirements.txt": 'private-analyzer==1.7.0\n',
                    "caller.py": 'from private_analyzer import analyze\n\ndef inspect(text):\n    return analyze(text)\n'},
                   "requirements.txt", 'private-analyzer==1.8.0\n',
                   ["The exact dependency source, target/intervening release notes and API compatibility contract are unavailable."],
                   "Report compatibility uncertainty for the unresolved external upgrade, without inventing a removed analyze API.")
    add_abstention("rust-external-tests-unknown", "rust", "testGap",
                   {"README.md": "# Rate parser\nThis bounded snapshot omits the separate integration-test repository and its coverage inventory. Both percent and ratio parsing are part of the public behavior contract.\n",
                    "src/rate.rs": 'pub fn percent(value: u8) -> f64 { f64::from(value) / 100.0 }\n'},
                   "src/rate.rs", 'pub fn percent(value: u8) -> f64 { f64::from(value) / 100.0 }\npub fn ratio(numerator: u8, denominator: u8) -> Option<f64> {\n    if denominator == 0 { None } else { Some(f64::from(numerator) / f64::from(denominator)) }\n}\n',
                   ["The external integration-test repository and coverage inventory were intentionally excluded from the supplied evidence."],
                   "Suggest checking denominator-zero coverage, but abstain from claiming that the absent external suite lacks it.")


def add_abstention(identifier: str, language: str, dimension: str,
                   base: dict[str, str], changed_file: str, replacement: str,
                   missing: list[str], acceptable: str) -> None:
    head = {**base, changed_file: replacement}
    base_path, head_path = ROOT / "fixtures" / identifier / "base", ROOT / "fixtures" / identifier / "head"
    write_tree(base_path, base)
    write_tree(head_path, head)
    diff = ROOT / "fixtures" / identifier / "change.diff"
    diff.write_text(make_diff(base, head), encoding="utf-8", newline="\n")
    CASES.append({"id": identifier, "pairId": None, "split": "holdout", "slice": "core",
                  "language": language, "expectation": "abstain", "dimension": dimension,
                  "base": f"fixtures/{identifier}/base", "head": f"fixtures/{identifier}/head",
                  "diff": f"fixtures/{identifier}/change.diff",
                  "source": {"kind": "authored", "license": "MIT",
                             "description": "Original authored bounded-evidence case; absent private/external context is explicit, not an observed defect.",
                             "baseSha256": tree_hash(base_path), "headSha256": tree_hash(head_path),
                             "diffSha256": hashlib.sha256(diff.read_bytes()).hexdigest()},
                  "issues": [], "abstention": {"missingEvidence": missing, "acceptableResponse": acceptable},
                  "reproducer": None, "annotation": {"status": "pending", "reviewer": None,
                  "rationale": "Separate review must verify that the missing evidence prevents the proposed concrete finding."}})


def main() -> None:
    build()
    manifest_path = ROOT / "manifest.json"
    # Bind the review to the oracle as well as the source content identities.
    old_cases = {}
    if manifest_path.exists():
        old_cases = {case["id"]: case for case in json.loads(manifest_path.read_text())["cases"]}
    for case in CASES:
        previous = old_cases.get(case["id"])
        if previous and {key: value for key, value in previous.items() if key != "annotation"} == {key: value for key, value in case.items() if key != "annotation"}:
            case["annotation"] = previous["annotation"]
    # Development controls are owned by the integrating agent and must not disappear.
    CASES.extend(case for case in old_cases.values() if case["split"] == "development")
    manifest = {"schemaVersion": 1, "datasetVersion": "momus-owned-v1", "cases": CASES}
    manifest_path.write_text(json.dumps(manifest, indent=2, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"Authored {len(CASES)} cases; {sum(c['expectation'] == 'defect' for c in CASES)} issue annotations remain subject to independent validation.")


if __name__ == "__main__":
    main()
