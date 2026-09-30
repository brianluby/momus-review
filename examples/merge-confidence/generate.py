#!/usr/bin/env python3
"""Regenerate deterministic synthetic fixtures. No network or real outcome data."""
import json
from pathlib import Path


def write(name, value):
    (Path(__file__).parent / name).write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


records = []
for i in range(100):
    training = i < 60
    occurred = i >= (40 if training else 80)
    merged_at = 100 + i if training else 2000 + i
    label = {
        "occurred": occurred,
        "observedAt": merged_at + 100,
        "evidence": f"fixture://synthetic/surveillance/{i}; fabricated, not observed history",
    }
    records.append({
        "head": f"synthetic-{i:03}",
        "mergedAt": merged_at,
        "scoreRecordedAt": merged_at - 1,
        "heuristicScore": 0.9 if occurred else 0.0,
        "revert": label,
        "incident": label,
        "flake": label,
    })

write("synthetic-history.json", {
    "repository": "fixture/repository",
    "provenance": "generate.py deterministic fabricated data; no real-world calibration",
    "synthetic": True,
    "asOf": 5000,
    "trainingCutoff": 1000,
    "windows": {"revertSeconds": 100, "incidentSeconds": 100, "flakeSeconds": 100},
    "records": records,
})
dimensions = ["correctness", "security", "reliability", "compatibility", "testGap"]
write("routine-report.json", {
    "mode": "changes",
    "scope": "synthetic routine change",
    "reviewedHead": "synthetic-candidate",
    "reviewedBase": "synthetic-base",
    "reviewedClean": True,
    "reviewedCommitted": True,
    "screenedFiles": 1,
    "dimensions": [{"key": key, "label": key, "short": key} for key in dimensions],
    "matrix": [{"file": "src/main.rs", **{key: 0.0 for key in dimensions}}],
    "findings": [],
    "partial": False,
})
write("opt-in-policy.json", {
    "enabled": True,
    "requiredChecks": ["test"],
    "maxRevertProbability": 0.1,
    "maxIncidentProbability": 0.1,
    "maxFlakeProbability": 0.1,
    "maxCalibrationError": 0.1,
    "maxFindingSeverity": 1.0,
    "maxCheckAgeSeconds": 3600,
    "maxHistoryAgeSeconds": 2592000,
})
write("synthetic-checks.json", {
    "repository": "fixture/repository",
    "head": "synthetic-candidate",
    "reviewedHead": "synthetic-candidate",
    "assessedAt": 5000,
    "reviewComplete": True,
    "evidenceComplete": True,
    "supportedInputs": True,
    "auxiliaryUnknowns": [],
    "checks": [{
        "name": "test",
        "head": "synthetic-candidate",
        "status": "passed",
        "completedAt": 4999,
        "evidence": "fixture://synthetic/check/test; fabricated check receipt",
    }],
})
