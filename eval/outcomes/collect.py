#!/usr/bin/env python3
"""Offline prospective collection; receipt authenticity is an operator trust boundary.

No model calls, GitHub writes, inference from absent telemetry, or score recomputation.
See docs/outcome-collection.md before using this with real repository evidence.
"""

import argparse
import hashlib
import json
import math
from pathlib import Path
import re
import sys
import time

KINDS = ("revert", "incident", "flake")
VERSION = 1
MAX_TIME = 2**63 - 1


class InvalidEvidence(ValueError):
    pass


def require(condition, message):
    if not condition:
        raise InvalidEvidence(message)


def fields(value, names, where):
    require(isinstance(value, dict), f"{where} must be an object")
    require(set(value) == set(names), f"{where} has missing or unknown fields")


def string(value, where):
    require(isinstance(value, str) and value.strip(), f"{where} must be nonempty text")


def integer(value, where, minimum=1):
    require(type(value) is int and minimum <= value <= MAX_TIME, f"{where} must be a bounded integer")


def head(value, where):
    require(isinstance(value, str) and re.fullmatch(r"(?:[0-9a-f]{40}|[0-9a-f]{64})", value)
            and set(value) != {"0"}, f"{where} must be a canonical full Git identity")


def sha(value, where):
    require(isinstance(value, str) and re.fullmatch(r"[0-9a-f]{64}", value), f"{where} must be SHA-256")


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False,
                      allow_nan=False).encode("utf-8")


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def no_duplicates(pairs):
    result = {}
    for key, value in pairs:
        require(key not in result, f"duplicate JSON field: {key}")
        result[key] = value
    return result


def parse(raw):
    try:
        return json.loads(raw, object_pairs_hook=no_duplicates,
                          parse_constant=lambda x: (_ for _ in ()).throw(InvalidEvidence(f"invalid number: {x}")))
    except (UnicodeDecodeError, json.JSONDecodeError) as error:
        raise InvalidEvidence(f"invalid UTF-8 JSON: {error}") from error


def load(path):
    return parse(Path(path).read_bytes())


def receipt(value, payload_digest, authority, now, producer=None):
    fields(value, ("authority", "uri", "sha256", "recordedAt", "producer", "verification"), "receipt")
    for key in ("authority", "uri", "producer", "verification"):
        string(value[key], f"receipt.{key}")
    sha(value["sha256"], "receipt.sha256")
    integer(value["recordedAt"], "receipt.recordedAt")
    require(value["sha256"] == payload_digest, "receipt digest does not bind the supplied bytes")
    require(value["authority"] == authority, "receipt authority is outside the registered protocol")
    require(value["recordedAt"] <= now, "receipt timestamp is in the future")
    if producer is not None:
        require(value["producer"] == producer, "score producer differs from registered pipeline")


def envelope(value, authority, now):
    fields(value, ("payload", "receipt"), "evidence envelope")
    receipt(value["receipt"], digest(canonical(value["payload"])), authority, now)
    return value["payload"]


def protocol(value, now):
    fields(value, ("payload", "receipt"), "protocol envelope")
    p = value["payload"]
    fields(p, ("schemaVersion", "repository", "heuristicVersion", "pipelineIdentity", "provenance",
               "protocolAuthority", "scoreAuthority", "mergeAuthority", "trainingCutoff", "windows"), "protocol")
    require(type(p["schemaVersion"]) is int and p["schemaVersion"] == VERSION, "unsupported protocol schemaVersion")
    require(type(p["heuristicVersion"]) is int and p["heuristicVersion"] == VERSION, "unsupported heuristicVersion")
    require(isinstance(p["repository"], str) and re.fullmatch(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+", p["repository"]),
            "repository must be an owner/name identity")
    for key in ("pipelineIdentity", "provenance", "protocolAuthority", "scoreAuthority", "mergeAuthority"):
        string(p[key], key)
    integer(p["trainingCutoff"], "trainingCutoff")
    fields(p["windows"], KINDS, "windows")
    for kind in KINDS:
        w = p["windows"][kind]
        fields(w, ("seconds", "definitionId", "definition", "authority"), f"windows.{kind}")
        integer(w["seconds"], f"{kind} window")
        for key in ("definitionId", "definition", "authority"):
            string(w[key], f"{kind}.{key}")
        require(w["authority"] != p["scoreAuthority"], "outcome telemetry must be independent of the score source")
    require(len({p["windows"][k]["definitionId"] for k in KINDS}) == len(KINDS),
            "outcomes require separate definition identities")
    envelope(value, p["protocolAuthority"], now)
    require(value["receipt"]["recordedAt"] < p["trainingCutoff"], "register the chronological cutoff before it occurs")
    return p


def report_score(report, p):
    require(isinstance(report, dict), "report must be an object")
    require(report.get("mode") == "changes" and report.get("partial") is False,
            "capture requires a complete changes review")
    require(report.get("reviewedClean") is True and report.get("reviewedCommitted") is True,
            "capture requires clean committed-object review provenance")
    require(type(report.get("skipped")) is list and not report["skipped"],
            "capture requires explicit absence of skipped review work")
    budget = report.get("budget")
    require(type(budget) is dict and type(budget.get("deferred")) is int and budget["deferred"] == 0,
            "capture requires explicit absence of deferred review work")
    require(report.get("shard") is None, "capture requires a complete unsharded review")
    # Momus records context clipping independently of partial. An affirmative
    # native drop counter defeats complete-review qualification even when the
    # report's matrix/counters otherwise cover every source subject. Older
    # stored reports need not invent fields absent from their producer schema.
    if "workflow" in report:
        workflow = report["workflow"]
        require(type(workflow) is dict, "native workflow must be an object")
        for name in ("droppedContextChars", "droppedContextItems"):
            if name in workflow:
                require(type(workflow[name]) is int and workflow[name] == 0,
                        f"capture requires absence of dropped review context: {name}")
    head(report.get("reviewedHead"), "report.reviewedHead")
    head(report.get("reviewedBase"), "report.reviewedBase")
    summary = report.get("mergeConfidence")
    require(isinstance(summary, dict), "report lacks a frozen mergeConfidence summary")
    require(type(summary.get("heuristicVersion")) is int and summary["heuristicVersion"] == p["heuristicVersion"],
            "report heuristicVersion is missing or incompatible")
    score = summary.get("heuristicScore")
    require(type(score) in (int, float) and 0 <= score <= 1 and math.isfinite(score),
            "heuristicScore must be finite in [0,1]")
    require(type(report.get("pRevert")) in (int, float) and report["pRevert"] == score,
            "report score fields disagree")
    return score


def capture(protocol_value, report_raw, report_receipt, context_value, now=None):
    """Freeze source bytes, not a retrospective re-review. `now` is a test clock."""
    now = int(time.time()) if now is None else now
    integer(now, "capture time")
    p = protocol(protocol_value, now)
    report = parse(report_raw)
    score = report_score(report, p)
    receipt(report_receipt, digest(report_raw), p["scoreAuthority"], now, p["pipelineIdentity"])
    context = envelope(context_value, p["mergeAuthority"], now)
    fields(context, ("repository", "pullRequest", "head", "base", "state", "mergedAt"), "open PR context")
    integer(context["pullRequest"], "pullRequest")
    require(context["repository"] == p["repository"], "PR context belongs to another repository")
    require(context["state"] == "OPEN" and context["mergedAt"] is None, "capture requires an open, unmerged PR receipt")
    require(context["head"] == report["reviewedHead"] and context["base"] == report["reviewedBase"],
            "PR head/base differs from the frozen report")
    require(protocol_value["receipt"]["recordedAt"] <= report_receipt["recordedAt"],
            "score predates the registered collection protocol")
    require(context_value["receipt"]["recordedAt"] >= report_receipt["recordedAt"],
            "open-PR source receipt must be at least as recent as the score receipt")
    return {
        "schemaVersion": VERSION,
        "protocolSha256": digest(canonical(protocol_value)),
        "capturedAt": now,
        "repository": p["repository"],
        "pullRequest": context["pullRequest"],
        "head": context["head"],
        "base": context["base"],
        "heuristicVersion": p["heuristicVersion"],
        "heuristicScore": score,
        "scoreRecordedAt": report_receipt["recordedAt"],
        "reportJson": report_raw.decode("utf-8"),
        "reportReceipt": report_receipt,
        "context": context_value,
    }


def validate_capture(value, protocol_value, now):
    fields(value, ("schemaVersion", "protocolSha256", "capturedAt", "repository", "pullRequest", "head", "base",
                   "heuristicVersion", "heuristicScore", "scoreRecordedAt", "reportJson", "reportReceipt", "context"),
           "frozen capture")
    integer(value["capturedAt"], "capturedAt")
    require(value["capturedAt"] <= now, "capture is in the future")
    string(value["reportJson"], "reportJson")
    reconstructed = capture(protocol_value, value["reportJson"].encode("utf-8"), value["reportReceipt"],
                            value["context"], value["capturedAt"])
    require(canonical(value) == canonical(reconstructed), "frozen capture was edited or uses another protocol")
    return value


def trusted_capture(value, protocol_value, now):
    p = protocol(protocol_value, now)
    c = envelope(value, p["scoreAuthority"], now)
    receipt(value["receipt"], digest(canonical(c)), p["scoreAuthority"], now, p["pipelineIdentity"])
    validate_capture(c, protocol_value, now)
    require(c["capturedAt"] <= value["receipt"]["recordedAt"], "capture source receipt predates capture")
    return c


def evidence_reference(receipt_value):
    return (f"{receipt_value['uri']}#sha256={receipt_value['sha256']}; "
            f"authority={receipt_value['authority']}; verification={receipt_value['verification']}")


def export_history(protocol_value, captures, merges, observations, as_of, now=None):
    """Join exact frozen heads and independently supplied observations, with audit."""
    now = int(time.time()) if now is None else now
    p = protocol(protocol_value, now)
    integer(as_of, "asOf")
    require(p["trainingCutoff"] < as_of <= now, "asOf must follow trainingCutoff and not be in the future")
    for value, name in ((captures, "captures"), (merges, "merges"), (observations, "observations")):
        require(isinstance(value, list), f"{name} must be an array")
    capture_by_pr = {}
    capture_heads = set()
    capture_receipts = {}
    for value in captures:
        c = trusted_capture(value, protocol_value, now)
        require(c["pullRequest"] not in capture_by_pr and c["head"] not in capture_heads,
                "ambiguous duplicate frozen PR/head")
        capture_by_pr[c["pullRequest"]] = c
        capture_receipts[c["pullRequest"]] = value["receipt"]
        capture_heads.add(c["head"])
    merge_by_pr = {}
    merge_heads = set()
    merge_commits = set()
    for value in merges:
        m = envelope(value, p["mergeAuthority"], now)
        fields(m, ("repository", "pullRequest", "head", "mergeCommit", "mergedAt"), "merged PR")
        require(m["repository"] == p["repository"], "merge belongs to another repository")
        integer(m["pullRequest"], "merge.pullRequest")
        head(m["head"], "merge.head")
        head(m["mergeCommit"], "merge.mergeCommit")
        integer(m["mergedAt"], "mergedAt")
        require(m["mergedAt"] <= value["receipt"]["recordedAt"], "merge receipt predates the merge")
        require(m["pullRequest"] not in merge_by_pr and m["head"] not in merge_heads
                and m["mergeCommit"] not in merge_commits, "ambiguous duplicate merge identity")
        merge_by_pr[m["pullRequest"]] = value
        merge_heads.add(m["head"])
        merge_commits.add(m["mergeCommit"])
    observations_by_key = {}
    for value in observations:
        fields(value, ("payload", "receipt"), "observation envelope")
        o = value["payload"]
        fields(o, ("repository", "pullRequest", "head", "kind", "definitionId", "occurred", "observedAt",
                   "availableAt", "coverageStart", "coverageEnd", "coverageComplete", "attribution"), "observation")
        require(o["kind"] in KINDS, "unknown outcome kind")
        w = p["windows"][o["kind"]]
        envelope(value, w["authority"], now)
        require(o["repository"] == p["repository"], "observation belongs to another repository")
        integer(o["pullRequest"], "observation.pullRequest")
        head(o["head"], "observation.head")
        require(o["definitionId"] == w["definitionId"], "observation definition differs from registered outcome")
        require(type(o["occurred"]) is bool, "occurred must be boolean")
        for key in ("observedAt", "availableAt"):
            integer(o[key], key)
        require(o["observedAt"] <= o["availableAt"] == value["receipt"]["recordedAt"],
                "label availability must be independently timestamped after the event/surveillance end")
        string(o["attribution"], "attribution")
        require(o["pullRequest"] in merge_by_pr, "observation lacks a matching authoritative merge")
        m = merge_by_pr[o["pullRequest"]]["payload"]
        require(o["head"] == m["head"], "observation head does not match merged PR head")
        maturity = m["mergedAt"] + w["seconds"]
        require(maturity <= MAX_TIME, "outcome window timestamp overflow")
        require(o["observedAt"] >= m["mergedAt"], "observation precedes merge")
        if o["occurred"]:
            require(o["observedAt"] <= maturity, "positive event falls outside its own window")
            require(o["coverageStart"] is None and o["coverageEnd"] is None and o["coverageComplete"] is None,
                    "positive event must not assert negative surveillance coverage")
        else:
            require(o["coverageComplete"] is True, "negative label requires an explicit complete-surveillance assertion")
            integer(o["coverageStart"], "coverageStart")
            integer(o["coverageEnd"], "coverageEnd")
            require(o["coverageStart"] <= m["mergedAt"] and o["coverageEnd"] >= maturity
                    and o["observedAt"] == o["coverageEnd"],
                    "negative label requires complete verified surveillance from merge through maturity")
        key = (o["pullRequest"], o["kind"])
        require(key not in observations_by_key, "duplicate/conflicting observation; adjudicate upstream")
        observations_by_key[key] = value
    history = {
        "heuristicVersion": p["heuristicVersion"], "repository": p["repository"],
        "provenance": f"{p['provenance']}; protocol {evidence_reference(protocol_value['receipt'])}",
        "synthetic": False, "asOf": as_of, "trainingCutoff": p["trainingCutoff"],
        "windows": {f"{k}Seconds": p["windows"][k]["seconds"] for k in KINDS}, "records": [],
    }
    audit = {"schemaVersion": VERSION, "protocolSha256": digest(canonical(protocol_value)),
             "asOf": as_of, "excludedMerges": [], "unmergedCaptures": [], "records": []}
    for number, merge_value in sorted(merge_by_pr.items()):
        m = merge_value["payload"]
        c = capture_by_pr.get(number)
        if c is None or m["mergedAt"] > as_of or merge_value["receipt"]["recordedAt"] > as_of:
            reason = "missing frozen score" if c is None else "merge unavailable by asOf"
            audit["excludedMerges"].append({"pullRequest": number, "head": m["head"], "reason": reason})
            continue
        require(c["head"] == m["head"], "frozen score belongs to a different PR revision")
        require(c["scoreRecordedAt"] < m["mergedAt"] and c["capturedAt"] < m["mergedAt"]
                and capture_receipts[number]["recordedAt"] < m["mergedAt"],
                "score/source capture must be strictly before merge; retrospective reconstruction is forbidden")
        record = {"head": m["head"], "mergedAt": m["mergedAt"], "scoreRecordedAt": c["scoreRecordedAt"],
                  "heuristicScore": c["heuristicScore"]}
        record_audit = {"pullRequest": number, "head": m["head"], "mergeCommit": m["mergeCommit"],
                        "scoreEvidence": evidence_reference(c["reportReceipt"]),
                        "captureEvidence": evidence_reference(capture_receipts[number]),
                        "mergeEvidence": evidence_reference(merge_value["receipt"]), "labels": {}}
        for kind in KINDS:
            value = observations_by_key.get((number, kind))
            record[kind] = None
            if value is None:
                record_audit["labels"][kind] = {"status": "unknown", "reason": "missing telemetry"}
                continue
            o = value["payload"]
            boundary = p["trainingCutoff"] if m["mergedAt"] < p["trainingCutoff"] else as_of
            reason = None
            if o["availableAt"] > boundary:
                reason = "label first available after trainingCutoff" if boundary != as_of else "label first available after asOf"
            elif m["mergedAt"] + p["windows"][kind]["seconds"] > boundary:
                reason = "outcome window immature at chronological boundary"
            record_audit["labels"][kind] = {"status": "withheld" if reason else "exported", "reason": reason,
                                             "availableAt": o["availableAt"], "observedAt": o["observedAt"],
                                             "evidence": evidence_reference(value["receipt"])}
            if reason is None:
                record[kind] = {"occurred": o["occurred"], "observedAt": o["observedAt"],
                                "evidence": evidence_reference(value["receipt"])}
        history["records"].append(record)
        audit["records"].append(record_audit)
    history["records"].sort(key=lambda r: (r["mergedAt"], r["head"]))
    for number, c in sorted(capture_by_pr.items()):
        if number not in merge_by_pr:
            audit["unmergedCaptures"].append({"pullRequest": number, "head": c["head"]})
    return history, audit


def write_new(path, value):
    """Never overwrite an earlier frozen capture, export, or audit."""
    with Path(path).open("x", encoding="utf-8") as output:
        json.dump(value, output, sort_keys=True, indent=2, ensure_ascii=False, allow_nan=False)
        output.write("\n")


def write_export(history_path, audit_path, history, audit):
    """Preserve the paired audit; roll back only files created by this call."""
    created = []
    try:
        for path, value in ((history_path, history), (audit_path, audit)):
            with Path(path).open("x", encoding="utf-8") as output:
                created.append(Path(path))
                json.dump(value, output, sort_keys=True, indent=2, ensure_ascii=False, allow_nan=False)
                output.write("\n")
    except BaseException:
        for path in reversed(created):
            path.unlink()
        raise


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    c = commands.add_parser("capture", help="freeze an existing trusted premerge report without recomputing its score")
    c.add_argument("--protocol", required=True)
    c.add_argument("--report", required=True)
    c.add_argument("--report-receipt", required=True)
    c.add_argument("--open-pr", required=True)
    c.add_argument("--output", required=True)
    v = commands.add_parser("validate", help="validate frozen source bytes and registered receipts")
    v.add_argument("--protocol", required=True)
    v.add_argument("--captures", required=True, help="JSON array of frozen-capture receipt envelopes")
    e = commands.add_parser("export", help="export strict OutcomeHistory and a separate availability audit")
    e.add_argument("--protocol", required=True)
    e.add_argument("--captures", required=True)
    e.add_argument("--merges", required=True)
    e.add_argument("--observations", required=True)
    e.add_argument("--as-of", required=True, type=int)
    e.add_argument("--output", required=True)
    e.add_argument("--audit", required=True)
    args = parser.parse_args(argv)
    try:
        p = load(args.protocol)
        if args.command == "capture":
            value = capture(p, Path(args.report).read_bytes(), load(args.report_receipt), load(args.open_pr))
            write_new(args.output, value)
            print("Frozen one premerge score; external receipt authenticity must already be verified.")
        elif args.command == "validate":
            now = int(time.time())
            protocol(p, now)
            captures = load(args.captures)
            require(isinstance(captures, list), "captures must be an array")
            identities = set()
            heads = set()
            for envelope_value in captures:
                value = trusted_capture(envelope_value, p, now)
                require(value["pullRequest"] not in identities and value["head"] not in heads,
                        "ambiguous duplicate frozen PR/head")
                identities.add(value["pullRequest"])
                heads.add(value["head"])
            print(f"Validated {len(captures)} frozen captures; no outcome inference performed.")
        else:
            require(Path(args.output).resolve() != Path(args.audit).resolve(), "history and audit require separate paths")
            require(not Path(args.output).exists() and not Path(args.audit).exists(), "refusing to overwrite an existing export")
            history, audit = export_history(p, load(args.captures), load(args.merges), load(args.observations), args.as_of)
            write_export(args.output, args.audit, history, audit)
            print(f"Exported {len(history['records'])} joined records; unknown/withheld labels stay null. No calibration claim.")
    except (InvalidEvidence, OSError, TypeError, ValueError) as error:
        print(f"invalid evidence: {error}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
