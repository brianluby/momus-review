#!/usr/bin/env python3
"""Validate and score the Momus-owned corpus without inference or dependencies.

Labels and adjudications are evaluator inputs, never review/model inputs. A
finding earns credit only through an independent, report-hash-bound issue
adjudication, after source identity and review completeness are verified.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
import re
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path, PurePosixPath, PureWindowsPath


CORE_DIMENSIONS = {"correctness", "security", "reliability", "compatibility", "testGap"}
DIMENSIONS = CORE_DIMENSIONS | {"docs", "dependency"}
QUALITY = ("sourceCorrectness", "specificity", "actionability", "uncertainty")
HASH_RE = re.compile(r"[0-9a-f]{64}\Z")
COMMIT_RE = re.compile(r"[0-9a-f]{40}\Z")
ID_RE = re.compile(r"[a-z0-9][a-z0-9._-]*\Z")


class ValidationError(ValueError):
    """Invalid evidence must never become a benchmark result."""


def fail(message):
    raise ValidationError(message)


def require(condition, message):
    if not condition:
        fail(message)


def object_keys(value, required, optional=(), label="object"):
    require(type(value) is dict, f"{label}: expected object")
    missing = set(required) - value.keys()
    extra = value.keys() - set(required) - set(optional)
    require(not missing, f"{label}: missing fields {sorted(missing)}")
    require(not extra, f"{label}: unknown fields {sorted(extra)}")


def string(value, label, nonempty=True):
    require(type(value) is str, f"{label}: expected string")
    require(not nonempty or bool(value.strip()), f"{label}: empty string")
    return value


def integer(value, label, minimum=0):
    require(type(value) is int and value >= minimum, f"{label}: expected integer >= {minimum}")
    return value


def boolean(value, label):
    require(type(value) is bool, f"{label}: expected boolean")
    return value


def sequence(value, label):
    require(type(value) is list, f"{label}: expected array")
    return value


def choice(value, choices, label):
    string(value, label)
    require(value in choices, f"{label}: unexpected value {value!r}")
    return value


def digest(value, label):
    require(type(value) is str and bool(HASH_RE.fullmatch(value)), f"{label}: expected lowercase SHA-256")
    return value


def commit(value, label):
    require(type(value) is str and bool(COMMIT_RE.fullmatch(value)), f"{label}: expected full lowercase Git commit")
    return value


def identifier(value, label):
    require(type(value) is str and bool(ID_RE.fullmatch(value)), f"{label}: invalid identifier")
    return value


def relative_path(value, label):
    string(value, label)
    path = PurePosixPath(value)
    require(not path.is_absolute() and not any(part in {".", "..", ".git"} for part in value.split("/")), f"{label}: unsafe relative path")
    require("\\" not in value and ":" not in value and "\x00" not in value, f"{label}: unsafe relative path")
    require(path.as_posix() == value and all(value.split("/")), f"{label}: path must be normalized POSIX")
    return value


def local_path(root, value, label):
    relative_path(value, label)
    root = Path(root).resolve()
    candidate = root / value
    for ancestor in [candidate, *candidate.parents]:
        if ancestor == root:
            break
        require(not ancestor.is_symlink(), f"{label}: symbolic links are forbidden")
    require(candidate.resolve().is_relative_to(root), f"{label}: path escapes evidence root")
    return candidate


def absolute_scope(value, label):
    """Check a recorded scope identity without accessing an archived workspace."""
    string(value, label)
    posix = PurePosixPath(value)
    windows = PureWindowsPath(value)
    # Archived native scopes may come from a different OS. Validate spelling
    # with pure path types; do not resolve or normalize the recorded identity.
    posix_valid = posix.is_absolute() and not value.startswith("//") and posix.as_posix() == value and ".." not in posix.parts
    # PureWindowsPath folds UNC server/share names into one anchor part, so
    # inspect raw components too; a share named '.' or '..' is not normalized.
    windows_components = value.split("\\")
    windows_valid = windows.is_absolute() and windows.root == "\\" and str(windows) == value and not {".", ".."}.intersection(windows_components)
    require((posix_valid or windows_valid) and "\x00" not in value,
            f"{label}: expected normalized absolute filesystem path")
    return value


def _pairs(items):
    result = {}
    for key, value in items:
        require(key not in result, f"JSON: duplicate object key {key!r}")
        result[key] = value
    return result


def load_json(path):
    """Read strict JSON: duplicate keys and non-finite numbers are rejected."""
    try:
        return json.loads(Path(path).read_text(encoding="utf-8"), object_pairs_hook=_pairs, parse_float=_finite_float,
                          parse_constant=lambda value: fail(f"JSON: invalid number {value}"))
    except ValidationError:
        raise
    except (OSError, UnicodeError, ValueError, RecursionError) as exc:
        fail(f"{path}: {exc}")


def _finite_float(value):
    number = float(value)
    require(math.isfinite(number), f"JSON: non-finite number {value}")
    return number


def sha256(path):
    try:
        return hashlib.sha256(Path(path).read_bytes()).hexdigest()
    except OSError as exc:
        fail(f"{path}: {exc}")


def tree_sha256(path):
    """Identity = SHA-256 of canonical sorted [{path, sha256}] UTF-8 JSON."""
    path = Path(path)
    require(path.is_dir(), f"{path}: source tree is missing")
    entries = []
    for item in sorted(path.rglob("*")):
        require(not item.is_symlink(), f"{item}: symbolic links are forbidden")
        if item.is_dir():
            continue
        require(item.is_file(), f"{item}: only regular source files are supported")
        relative = item.relative_to(path).as_posix()
        relative_path(relative, "source tree path")
        entries.append({"path": relative, "sha256": sha256(item)})
    require(entries, f"{path}: empty source tree")
    encoded = json.dumps(entries, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode("utf-8")
    return hashlib.sha256(encoded).hexdigest()


def source_hashes(source, label):
    for name in ("baseSha256", "headSha256", "diffSha256"):
        digest(source[name], f"{label}.{name}")


def validate_diff_applies(base, head_sha, diff, cid):
    """Apply exact diff bytes in a disposable tree; never execute fixture code."""
    try:
        text = diff.read_text(encoding="utf-8")
    except (OSError, UnicodeError) as exc:
        fail(f"{cid}: unreadable diff: {exc}")
    modes = re.findall(r"^(?:new file mode|old mode|new mode|deleted file mode) (\d+)$", text, re.MULTILINE)
    require(all(mode in {"100644", "100755"} for mode in modes), f"{cid}: diff may contain only regular files")
    executable = shutil.which("git")
    require(executable is not None, "git is required for offline diff validation")
    # A user shell's GIT_* identity must not redirect this disposable operation.
    environment = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    with tempfile.TemporaryDirectory(prefix="momus-diff-check-") as directory:
        workspace = Path(directory) / "source"
        shutil.copytree(base, workspace)
        try:
            applied = subprocess.run([executable, "-C", str(workspace), "apply", "--no-index", "--whitespace=nowarn", "--", str(diff.resolve())], env=environment, capture_output=True, text=True, timeout=30)
        except (OSError, subprocess.TimeoutExpired) as exc:
            fail(f"{cid}: offline diff application failed: {exc}")
        require(applied.returncode == 0, f"{cid}: diff does not apply to exact base: {applied.stderr.strip()[:500]}")
        require(tree_sha256(workspace) == head_sha, f"{cid}: applying diff does not reproduce exact head tree")


def validate_manifest(manifest_path, require_inventory=True):
    """Return a checked manifest; the optional inventory bypass is for tiny tests."""
    manifest_path = Path(manifest_path)
    data = load_json(manifest_path)
    object_keys(data, {"schemaVersion", "datasetVersion", "cases"}, label="manifest")
    require(type(data["schemaVersion"]) is int and data["schemaVersion"] == 1, "manifest: unsupported schemaVersion")
    identifier(data["datasetVersion"], "manifest.datasetVersion")
    cases = sequence(data["cases"], "manifest.cases")
    require(cases, "manifest: no cases")
    seen, pairs = set(), {}
    for case in cases:
        object_keys(case, {"id", "pairId", "split", "slice", "language", "expectation", "dimension", "base", "head", "diff", "source", "issues", "abstention", "reproducer", "annotation"}, label="case")
        cid = identifier(case["id"], "case.id")
        require(cid not in seen, f"duplicate case {cid}")
        seen.add(cid)
        choice(case["split"], {"holdout", "development"}, f"{cid}.split")
        choice(case["slice"], {"core", "auxiliary"}, f"{cid}.slice")
        choice(case["language"], {"rust", "javascript", "python", "docs", "dependency"}, f"{cid}.language")
        expectation = choice(case["expectation"], {"defect", "clean", "abstain"}, f"{cid}.expectation")
        dimension = choice(case["dimension"], DIMENSIONS, f"{cid}.dimension")
        require((dimension in CORE_DIMENSIONS) == (case["slice"] == "core"), f"{cid}: slice/dimension disagree")
        if case["pairId"] is not None:
            pair = identifier(case["pairId"], f"{cid}.pairId")
            pairs.setdefault(pair, []).append(case)
        require(expectation != "abstain" or case["pairId"] is None, f"{cid}: abstention cannot be a paired control")
        source = case["source"]
        object_keys(source, {"kind", "license", "description", "baseSha256", "headSha256", "diffSha256"}, {"repository", "revision"}, f"{cid}.source")
        choice(source["kind"], {"authored", "observed-control"}, f"{cid}.source.kind")
        string(source["license"], f"{cid}.source.license")
        string(source["description"], f"{cid}.source.description")
        if source["kind"] == "observed-control":
            require(case["split"] == "development", f"{cid}: exposed controls must be development cases")
            require("repository" in source and "revision" in source, f"{cid}: observed control needs repository and exact revision")
        if "repository" in source:
            string(source["repository"], f"{cid}.source.repository")
        if "revision" in source:
            commit(source["revision"], f"{cid}.source.revision")
        source_hashes(source, f"{cid}.source")
        base = local_path(manifest_path.parent, case["base"], f"{cid}.base")
        head = local_path(manifest_path.parent, case["head"], f"{cid}.head")
        diff = local_path(manifest_path.parent, case["diff"], f"{cid}.diff")
        require(tree_sha256(base) == source["baseSha256"], f"{cid}: base tree hash mismatch")
        require(tree_sha256(head) == source["headSha256"], f"{cid}: head tree hash mismatch")
        require(sha256(diff) == source["diffSha256"], f"{cid}: diff hash mismatch")
        require(diff.stat().st_size > 0 and source["baseSha256"] != source["headSha256"], f"{cid}: benchmark change must be nonempty")
        validate_diff_applies(base, source["headSha256"], diff, cid)
        issues = sequence(case["issues"], f"{cid}.issues")
        require(bool(issues) == (expectation == "defect"), f"{cid}: issues disagree with expectation")
        issue_ids = set()
        for issue in issues:
            object_keys(issue, {"id", "file", "startLine", "endLine", "dimension", "trigger", "contract", "consequence", "acceptableFinding", "explanation", "fix"}, label=f"{cid}.issue")
            iid = identifier(issue["id"], f"{cid}.issue.id")
            require(iid not in issue_ids, f"{cid}: duplicate issue {iid}")
            issue_ids.add(iid)
            file = local_path(head, issue["file"], f"{cid}.{iid}.file")
            require(file.is_file(), f"{cid}.{iid}: issue source file is missing")
            start = integer(issue["startLine"], f"{cid}.{iid}.startLine", 1)
            end = integer(issue["endLine"], f"{cid}.{iid}.endLine", start)
            try:
                lines = len(file.read_text(encoding="utf-8").splitlines())
            except (OSError, UnicodeError) as exc:
                fail(f"{cid}.{iid}: unreadable issue source: {exc}")
            require(end <= lines, f"{cid}.{iid}: issue location outside source")
            native = {"docs": "correctness", "dependency": "compatibility"}.get(dimension, dimension)
            require(issue["dimension"] == native, f"{cid}.{iid}: wrong native finding dimension")
            for name in ("trigger", "contract", "consequence", "acceptableFinding", "fix"):
                string(issue[name], f"{cid}.{iid}.{name}")
            object_keys(issue["explanation"], QUALITY, label=f"{cid}.{iid}.explanation")
            for name in QUALITY:
                string(issue["explanation"][name], f"{cid}.{iid}.explanation.{name}")
        abstention = case["abstention"]
        require((abstention is not None) == (expectation == "abstain"), f"{cid}: abstention disagrees with expectation")
        if abstention is not None:
            object_keys(abstention, {"missingEvidence", "acceptableResponse"}, label=f"{cid}.abstention")
            missing = sequence(abstention["missingEvidence"], f"{cid}.abstention.missingEvidence")
            require(missing, f"{cid}: missing evidence rationale is empty")
            for reason in missing:
                string(reason, f"{cid}.abstention.missingEvidence")
            string(abstention["acceptableResponse"], f"{cid}.abstention.acceptableResponse")
        if case["reproducer"] is not None:
            command = sequence(case["reproducer"], f"{cid}.reproducer")
            require(command, f"{cid}: reproducer argv cannot be empty")
            for arg in command:
                string(arg, f"{cid}.reproducer argument")
        annotation = case["annotation"]
        object_keys(annotation, {"status", "reviewer", "rationale"}, label=f"{cid}.annotation")
        choice(annotation["status"], {"pending", "validated"}, f"{cid}.annotation.status")
        string(annotation["rationale"], f"{cid}.annotation.rationale")
        if annotation["status"] == "validated":
            string(annotation["reviewer"], f"{cid}.annotation.reviewer")
        else:
            require(annotation["reviewer"] is None, f"{cid}: pending annotations cannot claim a reviewer")
    for pair_id, members in pairs.items():
        require(len(members) == 2 and {c["expectation"] for c in members} == {"defect", "clean"}, f"{pair_id}: pair requires exactly one defect and one fixed clean control")
        for field in ("split", "slice", "language", "dimension"):
            require(len({c[field] for c in members}) == 1, f"{pair_id}: pair {field} differs")
        require(len({c["source"]["headSha256"] for c in members}) == 2, f"{pair_id}: defect and fixed source must differ")
    if require_inventory:
        holdout = [c for c in cases if c["split"] == "holdout" and c["source"]["kind"] == "authored"]
        core = [c for c in holdout if c["slice"] == "core"]
        core_pairs = {c["pairId"] for c in core if c["pairId"] is not None}
        require(len(core_pairs) >= 12, "inventory: at least 12 authored core defect/fixed pairs required")
        require(sum(c["expectation"] == "abstain" for c in core) >= 4, "inventory: at least four insufficient-evidence cases required")
        require({c["language"] for c in core} >= {"rust", "javascript", "python"}, "inventory: Rust, JavaScript and Python required")
        require({c["dimension"] for c in core if c["expectation"] == "defect"} >= CORE_DIMENSIONS, "inventory: all five native review dimensions required")
        require({c["dimension"] for c in holdout if c["slice"] == "auxiliary" and c["expectation"] == "defect"} >= {"docs", "dependency"}, "inventory: separate docs and dependency defect controls required")
    return data


def number(value, label, maximum=None):
    # Python integers are finite, but converting an arbitrarily large integer
    # in math.isfinite() can overflow before the range check rejects it.
    require(type(value) in {int, float}, f"{label}: expected finite nonnegative number")
    require((type(value) is int or math.isfinite(value)) and value >= 0, f"{label}: expected finite nonnegative number")
    require(maximum is None or value <= maximum, f"{label}: number out of range")


def validate_finding(finding, label):
    required = {"file", "line", "dimension", "mechanism", "evidence"}
    optional = {"probability", "locationConfidence", "mechanismConfidence", "severity", "severityConfidence", "owner", "ownerConfidence", "action", "title", "why", "fix", "test", "testPlan", "fingerprint", "related", "rank", "taint", "exoneration", "ensemble"}
    object_keys(finding, required, optional, label)
    relative_path(finding["file"], f"{label}.file")
    integer(finding["line"], f"{label}.line")
    choice(finding["dimension"], CORE_DIMENSIONS, f"{label}.dimension")
    string(finding["mechanism"], f"{label}.mechanism")
    string(finding["evidence"], f"{label}.evidence", nonempty=False)
    for name in ("probability", "locationConfidence", "mechanismConfidence", "severityConfidence"):
        if name in finding:
            number(finding[name], f"{label}.{name}", 1)
    if "severity" in finding:
        number(finding["severity"], f"{label}.severity", 3)
    if finding.get("ownerConfidence") is not None:
        number(finding["ownerConfidence"], f"{label}.ownerConfidence", 1)
    for name in ("owner", "title", "why", "fix", "test"):
        if finding.get(name) is not None:
            string(finding[name], f"{label}.{name}", nonempty=False)
    if "action" in finding:
        choice(finding["action"], {"comment", "request_changes"}, f"{label}.action")
    if "fingerprint" in finding:
        string(finding["fingerprint"], f"{label}.fingerprint", nonempty=False)
    if finding.get("rank") is not None:
        integer(finding["rank"], f"{label}.rank", 1)
    if "related" in finding:
        for item in sequence(finding["related"], f"{label}.related"):
            object_keys(item, {"file", "line", "dimension", "mechanism", "confidence"}, label=f"{label}.related item")
            relative_path(item["file"], f"{label}.related.file")
            integer(item["line"], f"{label}.related.line")
            choice(item["dimension"], CORE_DIMENSIONS, f"{label}.related.dimension")
            string(item["mechanism"], f"{label}.related.mechanism")
            number(item["confidence"], f"{label}.related.confidence", 1)
    for name in ("testPlan", "taint", "exoneration", "ensemble"):
        if finding.get(name) is not None:
            require(type(finding[name]) is dict, f"{label}.{name}: expected object or null")


REPORT_FIELDS = {"reviewedHead", "reviewedBase", "reviewedClean", "reviewedCommitted", "upgrades", "docsDrift", "mergeConfidence", "mode", "scope", "dimensions", "config", "wallTimeMs", "screenedFiles", "contextFiles", "matrix", "followedSignals", "profiles", "workflow", "usage", "index", "budget", "followUpPlan", "specDrift", "partial", "tier", "shard", "redactions", "skipped", "findings", "pRevert"}


def validate_report(report, label):
    object_keys(report, {"partial", "skipped", "budget", "findings"}, REPORT_FIELDS, label)
    boolean(report["partial"], f"{label}.partial")
    for item in sequence(report["skipped"], f"{label}.skipped"):
        object_keys(item, {"file", "stage", "reason"}, label=f"{label}.skipped item")
        string(item["file"], f"{label}.skipped.file")
        choice(item["stage"], {"screen", "profile", "locate", "testplan"}, f"{label}.skipped.stage")
        string(item["reason"], f"{label}.skipped.reason")
    object_keys(report["budget"], {"deferred"}, {"limit", "reserved"}, f"{label}.budget")
    integer(report["budget"]["deferred"], f"{label}.budget.deferred")
    if report["budget"].get("limit") is not None:
        integer(report["budget"]["limit"], f"{label}.budget.limit")
    if "reserved" in report["budget"]:
        integer(report["budget"]["reserved"], f"{label}.budget.reserved")
    for index, finding in enumerate(sequence(report["findings"], f"{label}.findings")):
        validate_finding(finding, f"{label}.findings[{index}]")
    for name in ("reviewedHead", "reviewedBase"):
        if report.get(name) is not None:
            commit(report[name], f"{label}.{name}")
    for name in ("reviewedClean", "reviewedCommitted"):
        if name in report:
            boolean(report[name], f"{label}.{name}")
    for name in ("upgrades", "docsDrift"):
        if report.get(name) is not None:
            require(type(report[name]) is dict, f"{label}.{name}: expected object")
            work_name = "changes" if name == "upgrades" else "checks"
            object_keys(report[name], {"findings", "unknowns", work_name}, label=f"{label}.{name}")
            for item in sequence(report[name][work_name], f"{label}.{name}.{work_name}"):
                require(type(item) is dict, f"{label}.{name}.{work_name}: expected object entries")
            unknowns = sequence(report[name].get("unknowns", []), f"{label}.{name}.unknowns")
            for reason in unknowns:
                string(reason, f"{label}.{name}.unknowns")
            # Current Momus moves auxiliary findings into top-level findings.
            # Reject retained nested findings to prevent losing their alarms.
            require(not sequence(report[name].get("findings", []), f"{label}.{name}.findings"), f"{label}.{name}: normalize auxiliary findings into top-level findings before scoring")
    for name in ("wallTimeMs", "screenedFiles", "followedSignals"):
        if name in report:
            integer(report[name], f"{label}.{name}")
    if "pRevert" in report:
        number(report["pRevert"], f"{label}.pRevert", 1)
    for name in ("workflow", "usage", "index", "config", "redactions", "tier"):
        if name in report:
            require(type(report[name]) is dict, f"{label}.{name}: expected object")
    for name in ("mergeConfidence", "followUpPlan", "specDrift", "shard"):
        if report.get(name) is not None:
            require(type(report[name]) is dict, f"{label}.{name}: expected object or null")
    for name in ("dimensions", "contextFiles", "matrix", "profiles"):
        if name in report:
            sequence(report[name], f"{label}.{name}")
    if "dimensions" in report:
        seen = set()
        for item in report["dimensions"]:
            object_keys(item, {"key", "label", "short"}, label=f"{label}.dimension metadata")
            key = choice(item["key"], CORE_DIMENSIONS, f"{label}.dimension.key")
            require(key not in seen, f"{label}: duplicate dimension metadata")
            seen.add(key)
            string(item["label"], f"{label}.dimension.label")
            string(item["short"], f"{label}.dimension.short")
    if "matrix" in report:
        seen = set()
        for row in report["matrix"]:
            object_keys(row, {"file"}, CORE_DIMENSIONS, f"{label}.matrix row")
            relative_path(row["file"], f"{label}.matrix.file")
            require(row["file"] not in seen, f"{label}: duplicate matrix path")
            seen.add(row["file"])
            for dimension in row.keys() - {"file"}:
                number(row[dimension], f"{label}.matrix.{dimension}", 1)
    if "contextFiles" in report:
        seen = set()
        for path in report["contextFiles"]:
            relative_path(path, f"{label}.contextFiles")
            require(path not in seen, f"{label}: duplicate context path")
            seen.add(path)
    if "workflow" in report:
        for key, value in report["workflow"].items():
            integer(value, f"{label}.workflow.{key}")
    if "tier" in report and "dismissed" in report["tier"]:
        for path in sequence(report["tier"]["dismissed"], f"{label}.tier.dismissed"):
            relative_path(path, f"{label}.tier.dismissed")
    if "mode" in report:
        choice(report["mode"], {"changes", "codebase"}, f"{label}.mode")
    if "scope" in report:
        string(report["scope"], f"{label}.scope", nonempty=False)


def validate_receipt(path, expected_sha, manifest, manifest_sha, case):
    require(sha256(path) == expected_sha, f"{case['id']}: materialization receipt hash mismatch")
    receipt = load_json(path)
    require(type(receipt) is dict, "materialization receipt: expected object")
    version = receipt.get("schemaVersion")
    require(type(version) is int and version in {1, 2}, "receipt: unsupported schemaVersion")
    required = {"schemaVersion", "datasetVersion", "manifestSha256", "caseId", "source", "reviewedBase", "reviewedHead"}
    if version == 2:
        required.add("reviewedScope")
    object_keys(receipt, required, label="materialization receipt")
    require(receipt["datasetVersion"] == manifest["datasetVersion"] and receipt["manifestSha256"] == manifest_sha and receipt["caseId"] == case["id"], f"{case['id']}: receipt identity mismatch")
    object_keys(receipt["source"], {"baseSha256", "headSha256", "diffSha256"}, label="receipt.source")
    source_hashes(receipt["source"], "receipt.source")
    require(all(receipt["source"][key] == case["source"][key] for key in receipt["source"]), f"{case['id']}: receipt source mismatch")
    commit(receipt["reviewedBase"], "receipt.reviewedBase")
    commit(receipt["reviewedHead"], "receipt.reviewedHead")
    if version == 2:
        absolute_scope(receipt["reviewedScope"], "receipt.reviewedScope")
    return receipt


def validate_run(run_path, manifest, manifest_sha):
    run_path = Path(run_path)
    run = load_json(run_path)
    object_keys(run, {"schemaVersion", "datasetVersion", "manifestSha256", "runId", "kind", "cases"}, label="run")
    require(type(run["schemaVersion"]) is int and run["schemaVersion"] == 1, "run: unsupported schemaVersion")
    require(run["datasetVersion"] == manifest["datasetVersion"] and run["manifestSha256"] == manifest_sha, "run: manifest identity mismatch")
    identifier(run["runId"], "run.runId")
    choice(run["kind"], {"offline-test", "measured"}, "run.kind")
    manifest_cases = {case["id"]: case for case in manifest["cases"]}
    parsed = {}
    for entry in sequence(run["cases"], "run.cases"):
        object_keys(entry, {"caseId", "status", "report", "reportSha256", "receipt", "receiptSha256", "error", "abstention"}, label="run case")
        cid = identifier(entry["caseId"], "run case.caseId")
        require(cid in manifest_cases, f"run: unknown case {cid}")
        require(cid not in parsed, f"run: duplicate case {cid}")
        status = choice(entry["status"], {"complete", "partial", "failed", "missing"}, f"{cid}.status")
        if entry["error"] is not None:
            string(entry["error"], f"{cid}.error")
        require(status == "complete" or entry["error"] is not None, f"{cid}: incomplete case requires an explicit reason")
        require(status != "complete" or entry["error"] is None, f"{cid}: complete case cannot contain an error")
        abstention = entry["abstention"]
        if abstention is not None:
            object_keys(abstention, {"abstained", "rationale"}, label=f"{cid}.abstention")
            boolean(abstention["abstained"], f"{cid}.abstention.abstained")
            string(abstention["rationale"], f"{cid}.abstention.rationale")
        report = None
        reviewed_scope = "."
        if entry["report"] is None:
            require(status in {"failed", "missing"}, f"{cid}: report required for reviewed cases")
            require(all(entry[name] is None for name in ("reportSha256", "receipt", "receiptSha256")), f"{cid}: absent report cannot have evidence hashes")
            require(abstention is None, f"{cid}: no review report cannot establish abstention")
        else:
            require(status != "missing", f"{cid}: missing case cannot contain a report")
            digest(entry["reportSha256"], f"{cid}.reportSha256")
            report_path = local_path(run_path.parent, entry["report"], f"{cid}.report")
            require(sha256(report_path) == entry["reportSha256"], f"{cid}: report hash mismatch")
            report = load_json(report_path)
            validate_report(report, f"{cid}.report")
            digest(entry["receiptSha256"], f"{cid}.receiptSha256")
            receipt_path = local_path(run_path.parent, entry["receipt"], f"{cid}.receipt")
            receipt = validate_receipt(receipt_path, entry["receiptSha256"], manifest, manifest_sha, manifest_cases[cid])
            require(report.get("reviewedBase") == receipt["reviewedBase"] and report.get("reviewedHead") == receipt["reviewedHead"], f"{cid}: report reviewed identity differs from materialization receipt")
            reviewed_scope = receipt.get("reviewedScope", ".")
        parsed[cid] = {"entry": entry, "report": report, "reviewedScope": reviewed_scope}
    return run, parsed


def validate_adjudications(path, manifest, manifest_sha, run_path, run_cases):
    data = load_json(path)
    object_keys(data, {"schemaVersion", "datasetVersion", "manifestSha256", "runSha256", "reviewer", "cases"}, label="adjudications")
    require(type(data["schemaVersion"]) is int and data["schemaVersion"] == 1, "adjudications: unsupported schemaVersion")
    require(data["datasetVersion"] == manifest["datasetVersion"] and data["manifestSha256"] == manifest_sha and data["runSha256"] == sha256(run_path), "adjudications: evidence identity mismatch")
    reviewer = data["reviewer"]
    object_keys(reviewer, {"identity", "independent", "method"}, label="adjudications.reviewer")
    string(reviewer["identity"], "adjudications.reviewer.identity")
    require(boolean(reviewer["independent"], "adjudications.reviewer.independent"), "adjudications: independent source review required")
    choice(reviewer["method"], {"human-source-review", "offline-test"}, "adjudications.reviewer.method")
    manifest_cases = {case["id"]: case for case in manifest["cases"]}
    result = {}
    for case in sequence(data["cases"], "adjudications.cases"):
        object_keys(case, {"caseId", "reportSha256", "findings", "abstention"}, label="adjudication case")
        cid = identifier(case["caseId"], "adjudication.caseId")
        require(cid in run_cases and run_cases[cid]["report"] is not None, f"adjudication: {cid} has no report")
        require(cid not in result, f"adjudications: duplicate case {cid}")
        digest(case["reportSha256"], f"{cid}.adjudication.reportSha256")
        require(case["reportSha256"] == run_cases[cid]["entry"]["reportSha256"], f"{cid}: adjudication report hash mismatch")
        findings = run_cases[cid]["report"]["findings"]
        issue_ids = {issue["id"] for issue in manifest_cases[cid]["issues"]}
        annotated = {}
        for judgment in sequence(case["findings"], f"{cid}.adjudication.findings"):
            object_keys(judgment, {"index", "verdict", "issueId", "duplicateOf", "rationale", "explanation"}, label=f"{cid}.finding adjudication")
            index = integer(judgment["index"], f"{cid}.adjudication.index")
            require(index < len(findings), f"{cid}: adjudication finding index outside report")
            require(index not in annotated, f"{cid}: duplicate finding adjudication {index}")
            verdict = choice(judgment["verdict"], {"matched", "duplicate", "unrelated", "advisory", "unsupported"}, f"{cid}.adjudication.verdict")
            iid = judgment["issueId"]
            if verdict in {"matched", "duplicate"}:
                identifier(iid, f"{cid}.adjudication.issueId")
                require(iid in issue_ids, f"{cid}: adjudication names unknown issue {iid!r}")
            else:
                require(iid is None, f"{cid}: non-defect adjudication cannot name an issue")
            if verdict == "duplicate":
                original = integer(judgment["duplicateOf"], f"{cid}.duplicateOf")
                require(original < index, f"{cid}: duplicate must refer to an earlier finding")
            else:
                require(judgment["duplicateOf"] is None, f"{cid}: duplicateOf only allowed for duplicate verdicts")
            object_keys(judgment["rationale"], {"trigger", "contract", "consequence"}, label=f"{cid}.adjudication.rationale")
            for name in ("trigger", "contract", "consequence"):
                string(judgment["rationale"][name], f"{cid}.rationale.{name}")
            explanation = judgment["explanation"]
            object_keys(explanation, {*QUALITY, "rationale"}, label=f"{cid}.explanation")
            for name in QUALITY:
                boolean(explanation[name], f"{cid}.explanation.{name}")
            string(explanation["rationale"], f"{cid}.explanation.rationale")
            require(verdict not in {"matched", "duplicate", "advisory"} or explanation["sourceCorrectness"], f"{cid}: credited/advisory judgment must be source-correct")
            annotated[index] = judgment
        for index, judgment in annotated.items():
            if judgment["verdict"] == "duplicate":
                original = annotated.get(judgment["duplicateOf"])
                require(original is not None and original["verdict"] == "matched" and original["issueId"] == judgment["issueId"], f"{cid}: duplicate must refer to matched finding for the same issue")
        abstention = case["abstention"]
        if abstention is not None:
            object_keys(abstention, {"appropriate", "rationale"}, label=f"{cid}.abstention adjudication")
            boolean(abstention["appropriate"], f"{cid}.abstention.appropriate")
            string(abstention["rationale"], f"{cid}.abstention.rationale")
            require(run_cases[cid]["entry"]["abstention"] is not None, f"{cid}: abstention adjudication needs recorded abstention response")
        result[cid] = {"findings": annotated, "abstention": abstention}
    return data, result


def ratio(numerator, denominator):
    return None if denominator == 0 else numerator / denominator


def _changed_code_inventory(case, manifest_path):
    base = local_path(Path(manifest_path).parent, case["base"], "case.base")
    head = local_path(Path(manifest_path).parent, case["head"], "case.head")
    subjects, tests = set(), set()
    for item in head.rglob("*"):
        if not item.is_file() or item.suffix.lower() not in {".rs", ".py", ".pyi", ".pyw", ".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts"}:
            continue
        path = item.relative_to(head).as_posix()
        old = base / path
        if old.is_file() and old.read_bytes() == item.read_bytes():
            continue
        parts = PurePosixPath(path).parts
        name = parts[-1].lower()
        stem = item.stem.lower()
        test_path = any(p.lower() in {"test", "tests", "__tests__"} for p in parts[:-1])
        if item.suffix.lower() in {".js", ".jsx", ".mjs", ".cjs", ".ts", ".tsx", ".mts", ".cts"}:
            test_path |= ".test." in name or ".spec." in name or any(p.lower() == "spec" for p in parts[:-1])
        if item.suffix.lower() in {".py", ".pyi", ".pyw"}:
            test_path |= stem.startswith("test_") or stem.endswith("_test")
        if item.suffix.lower() == ".rs":
            test_path |= stem.endswith("_test")
        (tests if test_path else subjects).add(path)
    return subjects, tests


def report_attempt_incomplete(report, case, manifest_path, reviewed_scope="."):
    """Coverage/identity failures differ from an evidence-limited abstention."""
    if report.get("mode") != "changes" or report.get("scope") != reviewed_scope:
        return "benchmark requires changes mode over the exact receipt-bound materialized repository scope"
    if report["skipped"]:
        return "native report skipped review work"
    if report["budget"]["deferred"]:
        return "native report deferred budget work"
    if not report.get("reviewedClean", False):
        return "native report source cleanliness is unverified"
    if (case["slice"] == "core" or case["dimension"] == "dependency") and not report.get("reviewedCommitted", False):
        return "native source report committed source verification is incomplete"
    if report.get("shard") is not None:
        return "unmerged review shard"
    if report.get("tier", {}).get("dismissed"):
        return "native report dismissed review subjects"
    workflow = report.get("workflow", {})
    if workflow.get("droppedContextChars", 0) or workflow.get("droppedContextItems", 0):
        return "native report dropped review context"
    if workflow.get("followedSignals", 0) < workflow.get("thresholdSignals", 0):
        return "native report did not follow every threshold signal"
    if case["dimension"] == "docs":
        summary = report.get("docsDrift")
        if not summary or not summary.get("checks"):
            return "native docsDrift did not record supported auxiliary checks"
    else:
        expected_paths, expected_tests = _changed_code_inventory(case, manifest_path)
        rows = report.get("matrix")
        if rows is None or {row["file"] for row in rows} != expected_paths:
            return "native screening inventory differs from changed source paths"
        if any(set(row) != CORE_DIMENSIONS | {"file"} for row in rows):
            return "native screening matrix lacks one or more review dimensions"
        if {item["key"] for item in report.get("dimensions", [])} != CORE_DIMENSIONS:
            return "native dimension inventory is incomplete"
        if report.get("screenedFiles") != len(expected_paths) or workflow.get("screenedCells") != len(expected_paths) * len(CORE_DIMENSIONS):
            return "native screening counters do not establish full coverage"
        if set(report.get("contextFiles", [])) != expected_tests:
            return "native changed-test context inventory is incomplete"
    return None


def report_incomplete(report, case, manifest_path, reviewed_scope="."):
    attempt_error = report_attempt_incomplete(report, case, manifest_path, reviewed_scope)
    if attempt_error:
        return attempt_error
    if report["partial"]:
        return "native report is partial"
    for name in ("upgrades", "docsDrift"):
        if report.get(name) and report[name].get("unknowns"):
            return f"native {name} evidence is incomplete"
    return None


def score(manifest_path, run_path, adjudications_path, split="holdout", slice_name="all", require_inventory=True):
    manifest = validate_manifest(manifest_path, require_inventory)
    manifest_sha = sha256(manifest_path)
    run, run_cases = validate_run(run_path, manifest, manifest_sha)
    adjudications, judgments = validate_adjudications(adjudications_path, manifest, manifest_sha, run_path, run_cases)
    require(run["kind"] != "measured" or adjudications["reviewer"]["method"] == "human-source-review", "measured runs require independent human source review, not offline test adjudications")
    selected = [case for case in manifest["cases"] if (split == "all" or case["split"] == split) and (slice_name == "all" or case["slice"] == slice_name)]
    require(selected, "score selection contains no cases")
    totals = {key: 0 for key in ("cases", "completeCases", "incompleteCases", "missingCases", "failedCases", "unvalidatedCases", "issues", "caughtIssues", "missedIssues", "findings", "creditedFindings", "duplicateFindings", "falseAlarms", "advisories", "unadjudicatedFindings", "withheldMatches", "cleanCases", "completeCleanCases", "cleanCasesWithFalseAlarms", "cleanFalseAlarms", "abstentionCases", "appropriateAbstentions", "unexpectedAbstentions")}
    quality = {name: {"passed": 0, "assessed": 0} for name in QUALITY}
    case_results = []
    for case in selected:
        cid = case["id"]
        totals["cases"] += 1
        totals["issues"] += len(case["issues"])
        totals["cleanCases"] += int(case["expectation"] == "clean")
        totals["abstentionCases"] += int(case["expectation"] == "abstain")
        run_case = run_cases.get(cid)
        adjudication = judgments.get(cid, {"findings": {}, "abstention": None})
        report = run_case["report"] if run_case else None
        reviewed_scope = run_case["reviewedScope"] if run_case else "."
        findings = report["findings"] if report else []
        totals["findings"] += len(findings)
        status = run_case["entry"]["status"] if run_case else "missing"
        reason = run_case["entry"]["error"] if run_case else "case absent from run receipt"
        incomplete = reason if status != "complete" else report_incomplete(report, case, manifest_path, reviewed_scope)
        if case["annotation"]["status"] != "validated":
            incomplete = "corpus annotation has not been independently validated"
            totals["unvalidatedCases"] += 1
        unadjudicated = len(findings) - len(adjudication["findings"])
        totals["unadjudicatedFindings"] += unadjudicated
        if unadjudicated:
            incomplete = incomplete or "finding adjudication is incomplete"
        response = run_case["entry"]["abstention"] if run_case else None
        abstained = bool(response and response["abstained"])
        if abstained and case["expectation"] != "abstain":
            incomplete = incomplete or "review abstained on a case with sufficient authored evidence"
        complete = status == "complete" and not incomplete
        totals["completeCases"] += int(complete)
        totals["incompleteCases"] += int(not complete)
        totals["missingCases"] += int(status == "missing")
        totals["failedCases"] += int(status == "failed")
        totals["completeCleanCases"] += int(complete and case["expectation"] == "clean")
        issues = {issue["id"]: issue for issue in case["issues"]}
        caught, duplicate_indices, alarms, advisory_indices, withheld, invalid_matches = set(), [], [], [], [], []
        supported_issues = set()
        valid_indices = set()
        for index in sorted(adjudication["findings"]):
            judgment = adjudication["findings"][index]
            finding = findings[index]
            explanation = judgment["explanation"]
            for name in QUALITY:
                quality[name]["assessed"] += 1
                quality[name]["passed"] += int(explanation[name])
            verdict = judgment["verdict"]
            if verdict in {"matched", "duplicate"}:
                issue = issues[judgment["issueId"]]
                located = finding["file"] == issue["file"] and finding["dimension"] == issue["dimension"] and issue["startLine"] <= finding["line"] <= issue["endLine"]
                # Nonempty evidence and a source-correct independent rationale
                # are mandatory in addition to file/dimension/location.
                supported = located and bool(finding["evidence"].strip()) and explanation["sourceCorrectness"]
                if not supported or (verdict == "duplicate" and judgment["duplicateOf"] not in valid_indices):
                    alarms.append(index)
                    invalid_matches.append(index)
                    continue
                valid_indices.add(index)
                if verdict == "duplicate" or judgment["issueId"] in supported_issues:
                    duplicate_indices.append(index)
                elif complete:
                    caught.add(judgment["issueId"])
                else:
                    withheld.append(index)
                supported_issues.add(judgment["issueId"])
            elif verdict == "advisory":
                advisory_indices.append(index)
            else:
                alarms.append(index)
        totals["caughtIssues"] += len(caught)
        totals["creditedFindings"] += len(caught)
        totals["duplicateFindings"] += len(duplicate_indices)
        totals["falseAlarms"] += len(alarms)
        totals["advisories"] += len(advisory_indices)
        totals["withheldMatches"] += len(withheld)
        if case["expectation"] == "clean":
            totals["cleanFalseAlarms"] += len(alarms)
            totals["cleanCasesWithFalseAlarms"] += int(bool(alarms))
        attempt_complete = bool(report and status in {"complete", "partial"} and not report_attempt_incomplete(report, case, manifest_path, reviewed_scope) and case["annotation"]["status"] == "validated" and not unadjudicated)
        appropriate = bool(attempt_complete and case["expectation"] == "abstain" and abstained and not findings and adjudication["abstention"] and adjudication["abstention"]["appropriate"])
        totals["appropriateAbstentions"] += int(appropriate)
        totals["unexpectedAbstentions"] += int(abstained and case["expectation"] != "abstain")
        case_results.append({"caseId": cid, "expectation": case["expectation"], "slice": case["slice"], "dimension": case["dimension"], "status": status, "complete": complete, "attemptComplete": attempt_complete, "incompleteReason": incomplete, "caughtIssues": sorted(caught), "missedIssues": sorted(issues.keys() - caught), "falseAlarmIndices": alarms, "duplicateIndices": duplicate_indices, "advisoryIndices": advisory_indices, "invalidMatchIndices": invalid_matches, "withheldMatchIndices": withheld, "unadjudicatedIndices": sorted(set(range(len(findings))) - adjudication["findings"].keys()), "abstained": abstained, "appropriateAbstention": appropriate})
    totals["missedIssues"] = totals["issues"] - totals["caughtIssues"]
    fully_evaluated = totals["completeCases"] == totals["cases"]
    clean_evaluated = totals["completeCleanCases"] == totals["cleanCases"]
    abstention_evaluated = all(c["attemptComplete"] for c in case_results if c["expectation"] == "abstain")
    return {"schemaVersion": 1, "datasetVersion": manifest["datasetVersion"], "manifestSha256": manifest_sha, "runId": run["runId"], "runKind": run["kind"], "runSha256": sha256(run_path), "adjudicationsSha256": sha256(adjudications_path), "selection": {"split": split, "slice": slice_name}, "notice": "Deterministic offline scorer fixture; not measured product performance." if run["kind"] == "offline-test" else "Issue credit requires independently validated source labels and complete hash-bound source reviews.", "fullyEvaluated": fully_evaluated, "fullyEvaluatedCleanCases": clean_evaluated, "fullyEvaluatedAbstentionAttempts": abstention_evaluated, "totals": totals, "rates": {"issueRecall": ratio(totals["caughtIssues"], totals["issues"]), "cleanCaseFalseAlarmRate": ratio(totals["cleanCasesWithFalseAlarms"], totals["cleanCases"]) if clean_evaluated else None, "abstentionSuccessRate": ratio(totals["appropriateAbstentions"], totals["abstentionCases"]) if abstention_evaluated else None, "completionRate": ratio(totals["completeCases"], totals["cases"])}, "explanationQuality": {name: {**quality[name], "rate": ratio(quality[name]["passed"], quality[name]["assessed"])} for name in QUALITY}, "cases": case_results}


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    subparsers = parser.add_subparsers(dest="command", required=True)
    validate = subparsers.add_parser("validate", help="validate corpus schemas, source identities and minimum inventory")
    validate.add_argument("--manifest", type=Path, default=Path(__file__).with_name("manifest.json"))
    prepare = subparsers.add_parser("prepare", help="create blank run and adjudication templates; executes no reviews")
    prepare.add_argument("--manifest", type=Path, default=Path(__file__).with_name("manifest.json"))
    prepare.add_argument("--output", type=Path, required=True, help="new or empty evidence directory")
    scoring = subparsers.add_parser("score", help="score stored native reports against independent issue adjudications")
    scoring.add_argument("--manifest", type=Path, default=Path(__file__).with_name("manifest.json"))
    scoring.add_argument("--run", type=Path, required=True)
    scoring.add_argument("--adjudications", type=Path, required=True)
    scoring.add_argument("--split", choices=("holdout", "development", "all"), default="holdout")
    scoring.add_argument("--slice", dest="slice_name", choices=("core", "auxiliary", "all"), default="all")
    scoring.add_argument("--output", type=Path)
    args = parser.parse_args(argv)
    try:
        if args.command == "validate":
            manifest = validate_manifest(args.manifest)
            result = {"valid": True, "datasetVersion": manifest["datasetVersion"], "manifestSha256": sha256(args.manifest), "cases": len(manifest["cases"]), "validatedAnnotations": sum(c["annotation"]["status"] == "validated" for c in manifest["cases"])}
        elif args.command == "prepare":
            manifest = validate_manifest(args.manifest)
            require(not args.output.exists() or (args.output.is_dir() and not any(args.output.iterdir())), "prepare output must be a new or empty directory")
            args.output.mkdir(parents=True, exist_ok=True)
            manifest_sha = sha256(args.manifest)
            run = {"schemaVersion": 1, "datasetVersion": manifest["datasetVersion"], "manifestSha256": manifest_sha, "runId": "pending-run", "kind": "measured", "cases": [{"caseId": case["id"], "status": "missing", "report": None, "reportSha256": None, "receipt": None, "receiptSha256": None, "error": "review not executed", "abstention": None} for case in manifest["cases"]]}
            run_file = args.output / "run.json"
            run_file.write_text(json.dumps(run, indent=2, sort_keys=True) + "\n", encoding="utf-8")
            adjudications = {"schemaVersion": 1, "datasetVersion": manifest["datasetVersion"], "manifestSha256": manifest_sha, "runSha256": sha256(run_file), "reviewer": {"identity": "pending", "independent": False, "method": "human-source-review"}, "cases": []}
            (args.output / "adjudications.json").write_text(json.dumps(adjudications, indent=2, sort_keys=True) + "\n", encoding="utf-8")
            print(f"Created blank run and adjudication templates in {args.output}. Independent reviewer attestation is pending; no review was executed.")
            return 0
        else:
            result = score(args.manifest, args.run, args.adjudications, args.split, args.slice_name)
        encoded = json.dumps(result, indent=2, sort_keys=True, ensure_ascii=False) + "\n"
        if getattr(args, "output", None):
            args.output.write_text(encoded, encoding="utf-8")
        else:
            sys.stdout.write(encoded)
    except (ValidationError, OSError) as exc:
        print(f"validation error: {exc}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
