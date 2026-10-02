"""Fabricated unit inputs exercise collection mechanics, never real calibration."""

import contextlib
import copy
import io
import json
from pathlib import Path
import tempfile
import unittest

import collect


def receipt(raw, authority, timestamp, producer="test-normalizer-v1"):
    return {"authority": authority, "uri": f"test-only://{authority}/{timestamp}",
            "sha256": collect.digest(raw), "recordedAt": timestamp, "producer": producer,
            "verification": "test-only fabricated verifier receipt; not observed data"}


def envelope(payload, authority, timestamp, producer="test-normalizer-v1"):
    return {"payload": payload, "receipt": receipt(collect.canonical(payload), authority, timestamp, producer)}


class CollectionTests(unittest.TestCase):
    def setUp(self):
        self.head = "a" * 40
        self.base = "b" * 40
        self.pipeline = "test-only-committed-review-sha256:123"
        p = {"schemaVersion": 1, "repository": "test-only/example", "heuristicVersion": 1,
             "pipelineIdentity": self.pipeline, "provenance": "fabricated unit-test protocol; not calibration",
             "protocolAuthority": "protocol", "scoreAuthority": "scores", "mergeAuthority": "github",
             "trainingCutoff": 1000,
             "windows": {k: {"seconds": seconds, "definitionId": f"{k}-v1", "definition": f"test-only {k}",
                              "authority": f"{k}-telemetry"}
                         for k, seconds in (("revert", 300), ("incident", 300), ("flake", 100))}}
        self.protocol = envelope(p, "protocol", 10)
        self.report = {"mode": "changes", "partial": False, "reviewedClean": True, "reviewedCommitted": True,
                       "skipped": [], "budget": {"deferred": 0}, "shard": None,
                       "reviewedHead": self.head, "reviewedBase": self.base, "pRevert": 0.3,
                       "mergeConfidence": {"heuristicVersion": 1, "heuristicScore": 0.3}}
        self.raw = collect.canonical(self.report)
        self.report_receipt = receipt(self.raw, "scores", 100, self.pipeline)
        self.context = envelope({"repository": "test-only/example", "pullRequest": 1, "head": self.head,
                                 "base": self.base, "state": "OPEN", "mergedAt": None}, "github", 101)
        self.frozen = collect.capture(self.protocol, self.raw, self.report_receipt, self.context, now=102)
        self.captures = [envelope(self.frozen, "scores", 103, self.pipeline)]
        self.merges = [envelope({"repository": "test-only/example", "pullRequest": 1, "head": self.head,
                                "mergeCommit": "c" * 40, "mergedAt": 200}, "github", 201)]

    def observation(self, kind="revert", occurred=True, observed=250, available=260):
        return envelope({"repository": "test-only/example", "pullRequest": 1, "head": self.head, "kind": kind,
                         "definitionId": f"{kind}-v1", "occurred": occurred, "observedAt": observed,
                         "availableAt": available, "coverageStart": None if occurred else 200,
                         "coverageEnd": None if occurred else observed,
                         "coverageComplete": None if occurred else True,
                         "attribution": "test-only independently adjudicated attribution/surveillance"},
                        f"{kind}-telemetry", available)

    def export(self, observations=(), **kwargs):
        args = {"protocol_value": self.protocol, "captures": self.captures, "merges": self.merges,
                "observations": list(observations), "as_of": 2000, "now": 3000}
        args.update(kwargs)
        return collect.export_history(**args)

    def assert_invalid(self, call):
        with self.assertRaises(collect.InvalidEvidence):
            call()

    def test_incomplete_native_work_cannot_be_frozen(self):
        for key, value in [("skipped", None), ("skipped", [{"file": "a.rs"}]),
                           ("budget", None), ("budget", {"deferred": 1}),
                           ("budget", {"deferred": False}), ("shard", {"index": 1})]:
            with self.subTest(key=key, value=value):
                report = copy.deepcopy(self.report)
                report[key] = value
                self.assert_invalid(lambda: collect.report_score(report, self.protocol["payload"]))

    def test_export_audit_failure_rolls_back_only_new_history(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            audit = root / "existing-audit.json"
            audit.write_text("preserve original audit")
            with self.assertRaises(FileExistsError):
                collect.write_export(root / "history.json", audit, {}, {})
            self.assertFalse((root / "history.json").exists())
            self.assertEqual(audit.read_text(), "preserve original audit")
            with self.assertRaises(FileNotFoundError):
                collect.write_export(root / "history.json", root / "missing" / "audit.json", {}, {})
            self.assertFalse((root / "history.json").exists())
            collect.write_export(root / "history.json", root / "audit.json", {"records": []}, {"records": []})
            self.assertEqual(json.loads((root / "history.json").read_text()), {"records": []})
            self.assertTrue((root / "audit.json").exists())

    def test_freeze_preserves_exact_source_bytes_and_producer(self):
        self.assertEqual(self.frozen["reportJson"].encode(), self.raw)
        self.assertEqual(self.frozen["heuristicScore"], 0.3)
        self.assertEqual(self.frozen["scoreRecordedAt"], 100)
        self.assertEqual(collect.trusted_capture(self.captures[0], self.protocol, 2000), self.frozen)

    def test_native_dropped_context_never_qualifies_as_complete_capture(self):
        for name in ("droppedContextChars", "droppedContextItems"):
            for value in (1, 100, -1, True, "0", None):
                with self.subTest(name=name, value=value):
                    report = dict(self.report, workflow={name: value})
                    raw = collect.canonical(report)
                    self.assert_invalid(lambda: collect.capture(
                        self.protocol, raw, receipt(raw, "scores", 100, self.pipeline), self.context, now=102))

    def test_absent_optional_workflow_counters_and_explicit_zero_still_capture(self):
        for workflow in ({}, {"screenedCells": 5}, {"droppedContextChars": 0},
                         {"droppedContextChars": 0, "droppedContextItems": 0}):
            with self.subTest(workflow=workflow):
                report = dict(self.report, workflow=workflow)
                raw = collect.canonical(report)
                frozen = collect.capture(self.protocol, raw, receipt(raw, "scores", 100, self.pipeline),
                                         self.context, now=102)
                self.assertEqual(frozen["heuristicScore"], 0.3)
                self.assertEqual(frozen["reportJson"].encode(), raw)
        self.assertEqual(collect.capture(self.protocol, self.raw, self.report_receipt, self.context, now=102), self.frozen)

    def test_oversized_integer_score_has_controlled_api_and_cli_validation_error(self):
        report = copy.deepcopy(self.report)
        report["mergeConfidence"]["heuristicScore"] = 10**1000
        report["pRevert"] = 10**1000
        raw = collect.canonical(report)
        self.assert_invalid(lambda: collect.capture(self.protocol, raw, receipt(raw, "scores", 100, self.pipeline),
                                                    self.context, now=102))
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for name, value in (("protocol", self.protocol), ("report-receipt", receipt(raw, "scores", 100, self.pipeline)),
                                ("open-pr", self.context)):
                (root / f"{name}.json").write_bytes(collect.canonical(value))
            (root / "report.json").write_bytes(raw)
            output = root / "frozen.json"
            args = ["capture", "--protocol", str(root / "protocol.json"), "--report", str(root / "report.json"),
                    "--report-receipt", str(root / "report-receipt.json"), "--open-pr", str(root / "open-pr.json"),
                    "--output", str(output)]
            with contextlib.redirect_stderr(io.StringIO()) as error:
                self.assertEqual(collect.main(args), 2)
            self.assertIn("heuristicScore must be finite in [0,1]", error.getvalue())
            self.assertFalse(output.exists())

    def test_missing_telemetry_exports_three_unknowns_and_separate_merge_commit(self):
        history, audit = self.export()
        record = history["records"][0]
        self.assertFalse(history["synthetic"])
        self.assertEqual(set(history), {"heuristicVersion", "repository", "provenance", "synthetic", "asOf",
                                        "trainingCutoff", "windows", "records"})
        self.assertEqual(set(record), {"head", "mergedAt", "scoreRecordedAt", "heuristicScore", *collect.KINDS})
        self.assertEqual(record["head"], self.head)
        self.assertEqual(audit["records"][0]["mergeCommit"], "c" * 40)
        self.assertTrue(all(record[k] is None for k in collect.KINDS))
        self.assertTrue(all(audit["records"][0]["labels"][k]["reason"] == "missing telemetry" for k in collect.KINDS))

    def test_outcomes_have_independent_windows_labels_and_evidence(self):
        observations = [self.observation(), self.observation("incident", False, 500, 510),
                        self.observation("flake", False, 300, 310)]
        history, audit = self.export(observations)
        record = history["records"][0]
        self.assertTrue(record["revert"]["occurred"])
        self.assertFalse(record["incident"]["occurred"])
        self.assertFalse(record["flake"]["occurred"])
        self.assertEqual(history["windows"], {"revertSeconds": 300, "incidentSeconds": 300, "flakeSeconds": 100})
        self.assertEqual(len({record[k]["evidence"] for k in collect.KINDS}), 3)
        self.assertEqual(audit["records"][0]["labels"]["incident"]["availableAt"], 510)
        self.assertNotIn("availableAt", record["incident"])

    def test_event_before_cutoff_discovered_after_cutoff_never_trains(self):
        history, audit = self.export([self.observation(observed=250, available=1100)])
        self.assertIsNone(history["records"][0]["revert"])
        label = audit["records"][0]["labels"]["revert"]
        self.assertEqual(label["observedAt"], 250)
        self.assertEqual(label["availableAt"], 1100)
        self.assertEqual(label["reason"], "label first available after trainingCutoff")

    def test_negative_surveillance_learned_late_never_trains(self):
        history, _ = self.export([self.observation(occurred=False, observed=500, available=1100)])
        self.assertIsNone(history["records"][0]["revert"])

    def test_pre_cutoff_merge_with_immature_window_is_withheld(self):
        protocol = copy.deepcopy(self.protocol)
        protocol["payload"]["windows"]["revert"]["seconds"] = 900
        protocol = envelope(protocol["payload"], "protocol", 10)
        frozen = collect.capture(protocol, self.raw, self.report_receipt, self.context, now=102)
        history, audit = self.export([self.observation()], protocol_value=protocol,
                                     captures=[envelope(frozen, "scores", 103, self.pipeline)])
        self.assertIsNone(history["records"][0]["revert"])
        self.assertEqual(audit["records"][0]["labels"]["revert"]["reason"],
                         "outcome window immature at chronological boundary")

    def held_out(self):
        payload = dict(self.merges[0]["payload"], mergedAt=1100)
        return [envelope(payload, "github", 1101)]

    def test_held_out_event_unavailable_by_as_of_is_unknown(self):
        history, audit = self.export([self.observation(observed=1200, available=2100)], merges=self.held_out())
        self.assertIsNone(history["records"][0]["revert"])
        self.assertEqual(audit["records"][0]["labels"]["revert"]["reason"], "label first available after asOf")

    def test_held_out_early_positive_also_waits_for_full_window(self):
        history, audit = self.export([self.observation(observed=1200, available=1210)],
                                     merges=self.held_out(), as_of=1300)
        self.assertIsNone(history["records"][0]["revert"])
        self.assertEqual(audit["records"][0]["labels"]["revert"]["reason"],
                         "outcome window immature at chronological boundary")

    def test_mature_held_out_negative_requires_complete_surveillance(self):
        o = self.observation(occurred=False, observed=1400, available=1410)
        o["payload"]["coverageStart"] = 1100
        o = envelope(o["payload"], "revert-telemetry", 1410)
        history, _ = self.export([o], merges=self.held_out())
        self.assertFalse(history["records"][0]["revert"]["occurred"])

    def test_equal_cutoff_merge_is_held_out(self):
        merges = [envelope(dict(self.merges[0]["payload"], mergedAt=1000), "github", 1001)]
        history, _ = self.export([self.observation(observed=1100, available=1110)], merges=merges)
        self.assertTrue(history["records"][0]["revert"]["occurred"])

    def test_missing_score_is_excluded_and_unmerged_score_remains_in_audit(self):
        history, audit = self.export(captures=[])
        self.assertEqual(history["records"], [])
        self.assertEqual(audit["excludedMerges"][0]["reason"], "missing frozen score")
        history, audit = self.export(merges=[])
        self.assertEqual(history["records"], [])
        self.assertEqual(audit["unmergedCaptures"], [{"pullRequest": 1, "head": self.head}])

    def test_late_merge_source_receipt_is_excluded_at_historical_as_of(self):
        merges = [envelope(self.merges[0]["payload"], "github", 2100)]
        history, audit = self.export(merges=merges)
        self.assertEqual(history["records"], [])
        self.assertEqual(audit["excludedMerges"][0]["reason"], "merge unavailable by asOf")

    def test_frozen_capture_requires_a_timestamp_receipt_before_merge(self):
        for timestamp in (200, 201):
            with self.subTest(timestamp=timestamp):
                late = [envelope(self.frozen, "scores", timestamp, self.pipeline)]
                self.assert_invalid(lambda: self.export(captures=late))

    def test_hindsight_local_capture_is_rejected_even_with_earlier_report(self):
        frozen = collect.capture(self.protocol, self.raw, self.report_receipt, self.context, now=210)
        late = [envelope(frozen, "scores", 211, self.pipeline)]
        self.assert_invalid(lambda: self.export(captures=late))

    def test_frozen_source_edit_and_score_edit_are_rejected(self):
        for edit in (lambda c: c["payload"].update(heuristicScore=0.0),
                     lambda c: c["payload"].update(reportJson=c["payload"]["reportJson"] + " "),
                     lambda c: c["payload"].update(capturedAt=50),
                     lambda c: c["payload"].update(schemaVersion=True)):
            changed = copy.deepcopy(self.captures)
            edit(changed[0])
            self.assert_invalid(lambda: self.export(captures=changed))
            # Even replacing the envelope digest cannot hide disagreement with
            # the separately bound raw source report and reconstructed capture.
            changed[0] = envelope(changed[0]["payload"], "scores", 103, self.pipeline)
            self.assert_invalid(lambda: self.export(captures=changed))

    def test_report_trust_requirements_and_legacy_producer_are_rejected(self):
        for key, value in (("partial", True), ("reviewedClean", False), ("reviewedCommitted", False),
                           ("mode", "wholeRepo"), ("reviewedHead", "short-head"), ("reviewedBase", "0" * 40),
                           ("pRevert", 0.4)):
            with self.subTest(key=key):
                report = dict(self.report, **{key: value})
                raw = collect.canonical(report)
                self.assert_invalid(lambda: collect.capture(self.protocol, raw, receipt(raw, "scores", 100, self.pipeline),
                                                            self.context, now=102))
        for version in (None, 0, 2, True):
            report = copy.deepcopy(self.report)
            report["mergeConfidence"]["heuristicVersion"] = version
            raw = collect.canonical(report)
            self.assert_invalid(lambda: collect.capture(self.protocol, raw, receipt(raw, "scores", 100, self.pipeline),
                                                        self.context, now=102))

    def test_receipt_digest_authority_pipeline_future_and_registration(self):
        for key, value in (("sha256", "0" * 64), ("authority", "PR-authored-score"),
                           ("producer", "different-pipeline"), ("recordedAt", 103), ("recordedAt", 9),
                           ("verification", "")):
            with self.subTest(key=key, value=value):
                r = dict(self.report_receipt, **{key: value})
                self.assert_invalid(lambda: collect.capture(self.protocol, self.raw, r, self.context, now=102))

    def test_changed_protocol_or_adaptive_cutoff_is_rejected(self):
        p = copy.deepcopy(self.protocol)
        p["payload"]["trainingCutoff"] = 1100
        p = envelope(p["payload"], "protocol", 10)
        self.assert_invalid(lambda: self.export(protocol_value=p))
        p = envelope(self.protocol["payload"], "protocol", 1000)
        self.assert_invalid(lambda: self.export(protocol_value=p))

    def test_closed_or_different_revision_context_is_rejected(self):
        for key, value in (("state", "MERGED"), ("mergedAt", 200), ("head", "d" * 40),
                           ("base", "d" * 40), ("repository", "another/repo")):
            context = envelope(dict(self.context["payload"], **{key: value}), "github", 101)
            self.assert_invalid(lambda: collect.capture(self.protocol, self.raw, self.report_receipt, context, now=102))

    def test_merge_head_revision_mismatch_and_duplicates_are_rejected(self):
        merge = envelope(dict(self.merges[0]["payload"], head="d" * 40), "github", 201)
        self.assert_invalid(lambda: self.export(merges=[merge]))
        self.assert_invalid(lambda: self.export(merges=self.merges * 2))
        self.assert_invalid(lambda: self.export(captures=self.captures * 2))
        self.assert_invalid(lambda: self.export([self.observation()] * 2))

    def test_label_kind_definition_source_and_exact_head_are_required(self):
        for key, value in (("kind", "risk"), ("definitionId", "incident-v1"), ("head", "d" * 40),
                           ("repository", "another/repo"), ("attribution", ""), ("occurred", 0)):
            o = envelope(dict(self.observation()["payload"], **{key: value}), "revert-telemetry", 260)
            self.assert_invalid(lambda: self.export([o]))
        o = self.observation()
        o["receipt"]["authority"] = "scores"
        self.assert_invalid(lambda: self.export([o]))
        self.assert_invalid(lambda: self.export([self.observation()], merges=[]))

    def test_positive_event_outside_window_or_before_merge_is_rejected(self):
        for timestamp in (199, 501):
            self.assert_invalid(lambda: self.export([self.observation(observed=timestamp, available=600)]))

    def test_negative_incomplete_surveillance_is_rejected(self):
        for key, value in (("coverageStart", 201), ("coverageEnd", 499), ("coverageEnd", None),
                           ("observedAt", 499), ("coverageStart", True), ("coverageComplete", False)):
            o = self.observation(occurred=False, observed=500, available=510)
            o = envelope(dict(o["payload"], **{key: value}), "revert-telemetry", 510)
            self.assert_invalid(lambda: self.export([o]))

    def test_event_time_and_availability_receipt_cannot_be_conflated(self):
        o = self.observation(observed=250, available=260)
        o = envelope(dict(o["payload"], availableAt=250), "revert-telemetry", 260)
        self.assert_invalid(lambda: self.export([o]))
        self.assert_invalid(lambda: self.export([self.observation(observed=250, available=249)]))

    def test_overflow_and_unknown_fields_are_rejected(self):
        p = copy.deepcopy(self.protocol["payload"])
        p["windows"]["revert"]["seconds"] = collect.MAX_TIME
        p = envelope(p, "protocol", 10)
        frozen = collect.capture(p, self.raw, self.report_receipt, self.context, now=102)
        self.assert_invalid(lambda: self.export([self.observation()], protocol_value=p,
                                                captures=[envelope(frozen, "scores", 103, self.pipeline)]))
        o = self.observation()
        o = envelope(dict(o["payload"], inferredNegative=True), "revert-telemetry", 260)
        self.assert_invalid(lambda: self.export([o]))

    def test_json_duplicates_nonfinite_numbers_and_malformed_inputs(self):
        for raw in (b'{"a":1,"a":2}', b'{"x":NaN}', b'{"x":Infinity}', b'{', b'\xff'):
            self.assert_invalid(lambda: collect.parse(raw))
        self.assert_invalid(lambda: self.export(captures={}))
        self.assert_invalid(lambda: self.export(as_of=True))
        self.assert_invalid(lambda: self.export(as_of=1000))
        self.assert_invalid(lambda: self.export(as_of=3001))

    def test_export_deterministic_across_source_order(self):
        c2 = copy.deepcopy(self.captures[0]["payload"])
        report = dict(self.report, reviewedHead="d" * 40)
        raw = collect.canonical(report)
        context = envelope(dict(self.context["payload"], pullRequest=2, head="d" * 40), "github", 101)
        c2 = collect.capture(self.protocol, raw, receipt(raw, "scores", 100, self.pipeline), context, now=102)
        captures = self.captures + [envelope(c2, "scores", 103, self.pipeline)]
        merges = self.merges + [envelope(dict(self.merges[0]["payload"], pullRequest=2, head="d" * 40,
                                             mergeCommit="e" * 40, mergedAt=210), "github", 211)]
        observations = [self.observation(), self.observation("flake", False, 300, 310)]
        self.assertEqual(self.export(observations, captures=captures, merges=merges),
                         self.export(list(reversed(observations)), captures=list(reversed(captures)),
                                     merges=list(reversed(merges))))

    def test_cli_error_never_writes_history_and_freezes_are_exclusive(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            for name, value in (("protocol", self.protocol), ("captures", self.captures),
                                ("merges", self.merges), ("observations", [self.observation()] * 2)):
                (root / f"{name}.json").write_bytes(collect.canonical(value))
            args = ["export", "--protocol", str(root / "protocol.json"), "--captures", str(root / "captures.json"),
                    "--merges", str(root / "merges.json"), "--observations", str(root / "observations.json"),
                    "--as-of", "2000", "--output", str(root / "history.json"), "--audit", str(root / "audit.json")]
            with contextlib.redirect_stderr(io.StringIO()):
                self.assertEqual(collect.main(args), 2)
            self.assertFalse((root / "history.json").exists())
            self.assertFalse((root / "audit.json").exists())
            path = root / "frozen.json"
            collect.write_new(path, self.frozen)
            before = path.read_bytes()
            with self.assertRaises(FileExistsError):
                collect.write_new(path, {})
            self.assertEqual(path.read_bytes(), before)


if __name__ == "__main__":
    unittest.main()
