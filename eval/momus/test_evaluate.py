"""Offline regression checks using tiny stored native reports, never inference."""

import copy
import contextlib
import io
import json
from pathlib import Path
import shutil
import tempfile
import unittest
from unittest import mock

import evaluate


HERE = Path(__file__).resolve().parent


class ScorerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "stored"
        shutil.copytree(HERE / "stored", self.root)
        self.manifest_path = self.root / "manifest.json"
        self.run_path = self.root / "run.json"
        self.adj_path = self.root / "adjudications.json"
        self.manifest = evaluate.load_json(self.manifest_path)
        self.run = evaluate.load_json(self.run_path)
        self.adj = evaluate.load_json(self.adj_path)

    def write(self, path, value):
        path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")

    def entry(self, cid="unit-defect"):
        return next(c for c in self.run["cases"] if c["caseId"] == cid)

    def adjudication(self, cid="unit-defect"):
        return next(c for c in self.adj["cases"] if c["caseId"] == cid)

    def report_path(self, cid="unit-defect"):
        return self.root / self.entry(cid)["report"]

    def report(self, cid="unit-defect"):
        return evaluate.load_json(self.report_path(cid))

    def sync(self, manifest=False):
        """Explicitly rebind changed test evidence, never repair source hashes."""
        if manifest:
            self.write(self.manifest_path, self.manifest)
            manifest_sha = evaluate.sha256(self.manifest_path)
            self.run["manifestSha256"] = manifest_sha
            self.adj["manifestSha256"] = manifest_sha
            for entry in self.run["cases"]:
                if entry["receipt"] is not None:
                    path = self.root / entry["receipt"]
                    receipt = evaluate.load_json(path)
                    receipt["manifestSha256"] = manifest_sha
                    self.write(path, receipt)
        for entry in self.run["cases"]:
            if entry["report"] is not None:
                entry["reportSha256"] = evaluate.sha256(self.root / entry["report"])
                entry["receiptSha256"] = evaluate.sha256(self.root / entry["receipt"])
                for adjudication in self.adj["cases"]:
                    if adjudication["caseId"] == entry["caseId"]:
                        adjudication["reportSha256"] = entry["reportSha256"]
        self.write(self.run_path, self.run)
        self.adj["runSha256"] = evaluate.sha256(self.run_path)
        self.write(self.adj_path, self.adj)

    def score(self, **kwargs):
        return evaluate.score(self.manifest_path, self.run_path, self.adj_path,
                              require_inventory=False, **kwargs)

    def result_case(self, cid, result=None):
        return next(c for c in (result or self.score())["cases"] if c["caseId"] == cid)

    def test_stored_result_is_exact_and_deterministic(self):
        expected = evaluate.load_json(self.root / "expected-score.json")
        self.assertEqual(self.score(), expected)
        self.assertEqual(self.score(), self.score())
        self.assertEqual(expected["totals"]["caughtIssues"], 1)
        self.assertEqual(expected["totals"]["falseAlarms"], 2)
        self.assertEqual(expected["totals"]["duplicateFindings"], 1)
        self.assertEqual(expected["totals"]["advisories"], 2)
        self.assertEqual(expected["runKind"], "offline-test")

    def test_full_product_inventory_and_source_hashes(self):
        manifest = evaluate.validate_manifest(HERE / "manifest.json")
        self.assertGreaterEqual(len(manifest["cases"]), 32)

    def test_small_unit_corpus_cannot_pass_product_inventory(self):
        with self.assertRaisesRegex(evaluate.ValidationError, "12 authored"):
            evaluate.validate_manifest(self.manifest_path)

    def test_same_file_and_dimension_without_issue_adjudication_is_not_detection(self):
        js = self.adjudication()["findings"]
        for index in (0, 1):
            js[index]["verdict"] = "unrelated"
            js[index]["issueId"] = None
            js[index]["duplicateOf"] = None
        self.sync()
        result = self.score()
        self.assertEqual(result["totals"]["caughtIssues"], 0)
        self.assertEqual(result["totals"]["falseAlarms"], 4)

    def test_structurally_wrong_match_is_an_alarm(self):
        original = self.report()
        for name, value in (("file", "other.py"), ("line", 1), ("dimension", "security"), ("evidence", "")):
            with self.subTest(name=name):
                report = copy.deepcopy(original)
                report["findings"][0][name] = value
                self.write(self.report_path(), report)
                self.sync()
                result = self.score()
                self.assertEqual(result["totals"]["caughtIssues"], 0)
                self.assertEqual(self.result_case("unit-defect", result)["invalidMatchIndices"], [0, 1])

    def test_two_matched_findings_for_one_issue_are_still_one_detection(self):
        second = self.adjudication()["findings"][1]
        second["verdict"] = "matched"
        second["duplicateOf"] = None
        self.sync()
        result = self.score()
        self.assertEqual(result["totals"]["caughtIssues"], 1)
        self.assertEqual(result["totals"]["duplicateFindings"], 1)

    def test_unrelated_findings_in_defect_cases_are_false_alarms(self):
        result = self.score()
        self.assertEqual(self.result_case("unit-defect", result)["falseAlarmIndices"], [2])
        self.assertEqual(result["totals"]["cleanFalseAlarms"], 1)
        self.assertEqual(result["rates"]["cleanCaseFalseAlarmRate"], .5)

    def test_missing_case_stays_in_full_issue_denominator(self):
        self.run["cases"] = [c for c in self.run["cases"] if c["caseId"] != "unit-defect"]
        self.adj["cases"] = [c for c in self.adj["cases"] if c["caseId"] != "unit-defect"]
        self.sync()
        result = self.score()
        self.assertEqual(result["totals"]["issues"], 2)
        self.assertEqual(result["totals"]["caughtIssues"], 0)
        self.assertEqual(result["totals"]["missingCases"], 1)
        self.assertEqual(result["rates"]["issueRecall"], 0)
        self.assertFalse(result["fullyEvaluated"])

    def test_missing_clean_case_cannot_report_zero_alarm_rate(self):
        self.run["cases"] = [c for c in self.run["cases"] if c["caseId"] != "unit-fixed"]
        self.adj["cases"] = [c for c in self.adj["cases"] if c["caseId"] != "unit-fixed"]
        self.sync()
        result = self.score()
        self.assertEqual(result["totals"]["cleanCases"], 2)
        self.assertIsNone(result["rates"]["cleanCaseFalseAlarmRate"])
        self.assertFalse(result["fullyEvaluatedCleanCases"])

    def test_native_partial_skipped_budget_or_identity_each_withhold_detection(self):
        original = self.report()
        variants = [dict(partial=True), dict(skipped=[dict(file="adapter.py", stage="locate", reason="offline error")]), dict(budget=dict(deferred=1)), dict(reviewedClean=False), dict(reviewedCommitted=False), dict(shard=dict(index=0, count=2))]
        for change in variants:
            with self.subTest(change=change):
                report = copy.deepcopy(original)
                report.update(change)
                self.write(self.report_path(), report)
                self.sync()
                result = self.score()
                self.assertEqual(result["totals"]["caughtIssues"], 0)
                self.assertEqual(result["totals"]["issues"], 2)
                self.assertEqual(result["totals"]["withheldMatches"], 1)
                self.assertEqual(result["totals"]["duplicateFindings"], 1)
                self.assertFalse(self.result_case("unit-defect", result)["complete"])

    def test_partial_run_status_withholds_complete_native_report(self):
        self.entry()["status"] = "partial"
        self.entry()["error"] = "some review work was not completed"
        self.sync()
        self.assertEqual(self.score()["totals"]["caughtIssues"], 0)

    def test_failed_case_without_report_is_explicit(self):
        entry = self.entry()
        entry.update(status="failed", error="offline simulated failure", report=None, reportSha256=None, receipt=None, receiptSha256=None)
        self.adj["cases"] = [c for c in self.adj["cases"] if c["caseId"] != "unit-defect"]
        self.sync()
        result = self.score()
        self.assertEqual(result["totals"]["failedCases"], 1)
        self.assertEqual(result["totals"]["missedIssues"], 2)

    def test_partial_native_abstention_is_separately_successful(self):
        report = self.report("unit-abstain")
        report["partial"] = True
        self.write(self.report_path("unit-abstain"), report)
        self.sync()
        result = self.score()
        case = self.result_case("unit-abstain", result)
        self.assertFalse(case["complete"])
        self.assertTrue(case["attemptComplete"])
        self.assertTrue(case["appropriateAbstention"])
        self.assertEqual(result["rates"]["abstentionSuccessRate"], 1)

    def test_budget_skipped_abstention_cannot_earn_success(self):
        report = self.report("unit-abstain")
        report["budget"]["deferred"] = 1
        self.write(self.report_path("unit-abstain"), report)
        self.sync()
        result = self.score()
        self.assertEqual(result["totals"]["appropriateAbstentions"], 0)
        self.assertIsNone(result["rates"]["abstentionSuccessRate"])

    def test_dropped_context_cannot_earn_defect_credit_even_when_native_not_partial(self):
        original = self.report()
        self.assertFalse(original["partial"])
        for counter in ("droppedContextChars", "droppedContextItems"):
            with self.subTest(counter=counter):
                report = copy.deepcopy(original)
                report["workflow"][counter] = 1
                self.write(self.report_path(), report)
                self.sync()
                result = self.score()
                case = self.result_case("unit-defect", result)
                self.assertFalse(case["complete"])
                self.assertFalse(case["attemptComplete"])
                self.assertEqual(case["incompleteReason"], "native report dropped review context")
                self.assertEqual(result["totals"]["issues"], 2)
                self.assertEqual(result["totals"]["caughtIssues"], 0)
                self.assertEqual(result["totals"]["missedIssues"], 2)
                self.assertEqual(result["totals"]["withheldMatches"], 1)

    def test_dropped_context_cannot_earn_abstention_success(self):
        original = self.report("unit-abstain")
        self.assertFalse(original["partial"])
        for counter in ("droppedContextChars", "droppedContextItems"):
            with self.subTest(counter=counter):
                report = copy.deepcopy(original)
                report["workflow"][counter] = 1
                self.write(self.report_path("unit-abstain"), report)
                self.sync()
                result = self.score()
                case = self.result_case("unit-abstain", result)
                self.assertFalse(case["complete"])
                self.assertFalse(case["attemptComplete"])
                self.assertFalse(case["appropriateAbstention"])
                self.assertEqual(result["totals"]["abstentionCases"], 1)
                self.assertEqual(result["totals"]["appropriateAbstentions"], 0)
                self.assertIsNone(result["rates"]["abstentionSuccessRate"])

    def test_abstention_requires_independent_rationale(self):
        self.adjudication("unit-abstain")["abstention"] = None
        self.sync()
        self.assertEqual(self.score()["totals"]["appropriateAbstentions"], 0)

    def test_auxiliary_exact_clean_worktree_can_score_without_committed_only(self):
        result = self.score(slice_name="auxiliary")
        self.assertEqual(result["totals"]["completeCases"], 2)
        self.assertEqual(result["totals"]["issues"], 1)
        self.assertEqual(result["totals"]["caughtIssues"], 0)

    def test_auxiliary_unknowns_withhold_detection(self):
        report = self.report("unit-doc-defect")
        report["findings"] = [dict(file="README.md", line=1, dimension="correctness", mechanism="docsContractDrift", evidence="send(message) lacks token")]
        report["docsDrift"]["unknowns"] = ["source identity incomplete"]
        self.write(self.report_path("unit-doc-defect"), report)
        judgment = copy.deepcopy(self.adjudication()["findings"][0])
        judgment["issueId"] = "unit-doc-issue"
        self.adjudication("unit-doc-defect")["findings"] = [judgment]
        self.sync()
        result = self.score(slice_name="auxiliary")
        self.assertEqual(result["totals"]["caughtIssues"], 0)
        self.assertEqual(result["totals"]["withheldMatches"], 1)

    def test_scan_or_restricted_scope_cannot_score_change_benchmark(self):
        original = self.report()
        for change in (dict(mode="codebase"), dict(scope="adapter.py")):
            report = copy.deepcopy(original)
            report.update(change)
            self.write(self.report_path(), report)
            self.sync()
            self.assertEqual(self.score()["totals"]["caughtIssues"], 0)

    def test_native_absolute_scope_matches_version_two_receipt_without_report_rewriting(self):
        receipt_path = self.root / self.entry()["receipt"]
        receipt = evaluate.load_json(receipt_path)
        scope = str((self.root / "materialized-repository").resolve())
        receipt.update(schemaVersion=2, reviewedScope=scope)
        self.write(receipt_path, receipt)
        report = self.report()
        report["scope"] = scope
        self.write(self.report_path(), report)
        report_bytes = self.report_path().read_bytes()
        self.sync()
        result = self.score()
        self.assertEqual(result["totals"]["caughtIssues"], 1)
        self.assertTrue(self.result_case("unit-defect", result)["complete"])
        self.assertTrue(self.result_case("unit-defect", result)["attemptComplete"])
        _, parsed = evaluate.validate_run(self.run_path, self.manifest, evaluate.sha256(self.manifest_path))
        self.assertEqual(parsed["unit-defect"]["reviewedScope"], scope)
        self.assertEqual(self.report_path().read_bytes(), report_bytes)

    def test_version_two_receipt_rejects_wrong_absolute_root_subpath_and_relative_scope(self):
        receipt_path = self.root / self.entry()["receipt"]
        receipt = evaluate.load_json(receipt_path)
        scope = str((self.root / "materialized-repository").resolve())
        receipt.update(schemaVersion=2, reviewedScope=scope)
        self.write(receipt_path, receipt)
        original = self.report()
        for reported_scope in (scope + "/src", str((self.root / "different-repository").resolve()), "."):
            with self.subTest(scope=reported_scope):
                report = copy.deepcopy(original)
                report["scope"] = reported_scope
                self.write(self.report_path(), report)
                self.sync()
                result = self.score()
                self.assertEqual(result["totals"]["issues"], 2)
                self.assertEqual(result["totals"]["caughtIssues"], 0)
                self.assertEqual(result["totals"]["missedIssues"], 2)
                case = self.result_case("unit-defect", result)
                self.assertFalse(case["complete"])
                self.assertFalse(case["attemptComplete"])
                self.assertIn("exact receipt-bound", case["incompleteReason"])

    def test_receipt_version_two_scope_requires_normalized_absolute_path(self):
        receipt_path = self.root / self.entry()["receipt"]
        original = evaluate.load_json(receipt_path)
        invalid = ("relative/repository", "/tmp/repository/../other", "/tmp/repository//src", "/tmp/repository/", "//tmp/repository", "/tmp/repository\x00", None, 123)
        for scope in invalid:
            with self.subTest(scope=scope):
                receipt = copy.deepcopy(original)
                receipt.update(schemaVersion=2, reviewedScope=scope)
                self.write(receipt_path, receipt)
                self.sync()
                with self.assertRaises(evaluate.ValidationError):
                    self.score()
        receipt = copy.deepcopy(original)
        receipt["schemaVersion"] = 2
        self.write(receipt_path, receipt)
        self.sync()
        with self.assertRaisesRegex(evaluate.ValidationError, "missing fields"):
            self.score()

    def test_canonical_windows_drive_and_unc_scopes_score_on_another_host(self):
        receipt_path = self.root / self.entry()["receipt"]
        original_receipt = evaluate.load_json(receipt_path)
        original_report = self.report()
        for scope in (r"C:\Users\reviewer\AppData\Local\Temp\momus-input", r"\\fileserver\review-share\momus-input"):
            with self.subTest(scope=scope):
                receipt = copy.deepcopy(original_receipt)
                receipt.update(schemaVersion=2, reviewedScope=scope)
                self.write(receipt_path, receipt)
                report = copy.deepcopy(original_report)
                report["scope"] = scope
                self.write(self.report_path(), report)
                self.sync()
                result = self.score()
                self.assertEqual(result["totals"]["caughtIssues"], 1)
                self.assertTrue(self.result_case("unit-defect", result)["complete"])

    def test_windows_scope_aliases_relative_paths_and_traversal_are_rejected(self):
        receipt_path = self.root / self.entry()["receipt"]
        original = evaluate.load_json(receipt_path)
        invalid = (r"C:relative", r"\root-relative", r"C:\Users\..\momus-input", r"C:\Users\.\momus-input", r"C:\\Users\momus-input", "C:/Users/reviewer/momus-input", r"\\server\share\..\momus-input", r"\\server\share\\momus-input", r"\\server\..\momus-input", r"\\server\.\momus-input", r"\\..\share\momus-input", r"\\.\share\momus-input", r"\\server")
        for scope in invalid:
            with self.subTest(scope=scope):
                receipt = copy.deepcopy(original)
                receipt.update(schemaVersion=2, reviewedScope=scope)
                self.write(receipt_path, receipt)
                self.sync()
                with self.assertRaises(evaluate.ValidationError):
                    self.score()

    def test_windows_receipt_scope_still_requires_exact_report_identity(self):
        receipt_path = self.root / self.entry()["receipt"]
        original_receipt = evaluate.load_json(receipt_path)
        original_report = self.report()
        mismatches = ((r"C:\Users\reviewer\momus-input", r"C:\Users\reviewer\other-input"), (r"C:\Users\reviewer\momus-input", r"C:\Users\reviewer\momus-input\src"), (r"\\fileserver\review-share\momus-input", r"\\fileserver\review-share\other-input"))
        for expected, reported in mismatches:
            with self.subTest(expected=expected, reported=reported):
                receipt = copy.deepcopy(original_receipt)
                receipt.update(schemaVersion=2, reviewedScope=expected)
                self.write(receipt_path, receipt)
                report = copy.deepcopy(original_report)
                report["scope"] = reported
                self.write(self.report_path(), report)
                self.sync()
                result = self.score()
                self.assertEqual(result["totals"]["issues"], 2)
                self.assertEqual(result["totals"]["caughtIssues"], 0)
                self.assertEqual(result["totals"]["missedIssues"], 2)
                self.assertFalse(self.result_case("unit-defect", result)["attemptComplete"])

    def test_legacy_version_one_scope_is_exact_and_does_not_accept_new_fields(self):
        _, parsed = evaluate.validate_run(self.run_path, self.manifest, evaluate.sha256(self.manifest_path))
        self.assertEqual(parsed["unit-defect"]["reviewedScope"], ".")
        report = self.report()
        report["scope"] = str(self.root.resolve())
        self.write(self.report_path(), report)
        self.sync()
        self.assertEqual(self.score()["totals"]["caughtIssues"], 0)
        receipt_path = self.root / self.entry()["receipt"]
        receipt = evaluate.load_json(receipt_path)
        receipt["reviewedScope"] = str(self.root.resolve())
        self.write(receipt_path, receipt)
        self.sync()
        with self.assertRaisesRegex(evaluate.ValidationError, "unknown fields"):
            self.score()

    def test_screening_coverage_gate_catches_omitted_file_and_dimension(self):
        original = self.report()
        omitted_file = copy.deepcopy(original)
        omitted_file["matrix"] = []
        omitted_dimension = copy.deepcopy(original)
        del omitted_dimension["matrix"][0]["security"]
        wrong_counters = copy.deepcopy(original)
        wrong_counters["workflow"]["screenedCells"] = 0
        for report in (omitted_file, omitted_dimension, wrong_counters):
            self.write(self.report_path(), report)
            self.sync()
            self.assertEqual(self.score()["totals"]["caughtIssues"], 0)

    def test_pending_annotation_never_earns_detection(self):
        self.manifest["cases"][0]["annotation"].update(status="pending", reviewer=None)
        self.sync(manifest=True)
        result = self.score()
        self.assertEqual(result["totals"]["unvalidatedCases"], 1)
        self.assertEqual(result["totals"]["caughtIssues"], 0)

    def test_missing_finding_adjudication_withholds_case_credit(self):
        self.adjudication()["findings"].pop()
        self.sync()
        result = self.score()
        self.assertEqual(result["totals"]["caughtIssues"], 0)
        self.assertEqual(result["totals"]["unadjudicatedFindings"], 1)

    def test_explanation_quality_is_separate_from_detection(self):
        self.adjudication()["findings"][0]["explanation"].update(specificity=False, actionability=False, uncertainty=False)
        self.sync()
        result = self.score()
        self.assertEqual(result["totals"]["caughtIssues"], 1)
        self.assertEqual(result["explanationQuality"]["actionability"]["assessed"], 6)
        self.assertLess(result["explanationQuality"]["actionability"]["rate"], 1)

    def test_report_tamper_is_rejected_before_scoring(self):
        self.report_path().write_text(self.report_path().read_text() + " ")
        with self.assertRaisesRegex(evaluate.ValidationError, "report hash mismatch"):
            self.score()

    def test_source_tamper_is_rejected_before_scoring(self):
        path = self.root / self.manifest["cases"][0]["head"] / "adapter.py"
        path.write_text(path.read_text() + "# changed source\n")
        with self.assertRaisesRegex(evaluate.ValidationError, "head tree hash mismatch"):
            self.score()

    def test_receipt_tamper_is_rejected(self):
        path = self.root / self.entry()["receipt"]
        path.write_text(path.read_text() + " ")
        with self.assertRaisesRegex(evaluate.ValidationError, "receipt hash mismatch"):
            self.score()

    def test_rebound_receipt_source_mismatch_is_rejected(self):
        path = self.root / self.entry()["receipt"]
        receipt = evaluate.load_json(path)
        receipt["source"]["headSha256"] = "0" * 64
        self.write(path, receipt)
        self.sync()
        with self.assertRaisesRegex(evaluate.ValidationError, "receipt source mismatch"):
            self.score()

    def test_rebound_report_reviewed_commit_mismatch_is_rejected(self):
        report = self.report()
        report["reviewedHead"] = "0" * 40
        self.write(self.report_path(), report)
        self.sync()
        with self.assertRaisesRegex(evaluate.ValidationError, "report reviewed identity"):
            self.score()

    def test_run_and_adjudication_hashes_are_bound(self):
        self.adj["runSha256"] = "0" * 64
        self.write(self.adj_path, self.adj)
        with self.assertRaisesRegex(evaluate.ValidationError, "evidence identity mismatch"):
            self.score()

    def test_adjudication_binds_exact_report_hash(self):
        self.adjudication()["reportSha256"] = "0" * 64
        self.write(self.adj_path, self.adj)
        with self.assertRaisesRegex(evaluate.ValidationError, "adjudication report hash mismatch"):
            self.score()

    def test_duplicate_case_and_finding_adjudication_are_rejected(self):
        self.run["cases"].append(copy.deepcopy(self.entry()))
        self.sync()
        with self.assertRaisesRegex(evaluate.ValidationError, "duplicate case"):
            self.score()
        self.run["cases"].pop()
        self.adjudication()["findings"].append(copy.deepcopy(self.adjudication()["findings"][0]))
        self.sync()
        with self.assertRaisesRegex(evaluate.ValidationError, "duplicate finding adjudication"):
            self.score()

    def test_wrong_issue_unknown_case_and_invalid_duplicate_are_rejected(self):
        original = copy.deepcopy(self.adjudication()["findings"])
        variants = [("issueId", "invented-issue"), ("index", 99), ("index", True), ("issueId", [])]
        for key, value in variants:
            with self.subTest(key=key, value=value):
                self.adjudication()["findings"] = copy.deepcopy(original)
                self.adjudication()["findings"][0][key] = value
                self.sync()
                with self.assertRaises(evaluate.ValidationError):
                    self.score()
        self.adjudication()["findings"] = copy.deepcopy(original)
        self.adjudication()["findings"][1]["duplicateOf"] = 1
        self.sync()
        with self.assertRaisesRegex(evaluate.ValidationError, "earlier finding"):
            self.score()

    def test_nonindependent_or_test_adjudication_cannot_authorize_measured_results(self):
        self.adj["reviewer"]["independent"] = False
        self.sync()
        with self.assertRaisesRegex(evaluate.ValidationError, "independent source review"):
            self.score()
        self.adj["reviewer"]["independent"] = True
        self.run["kind"] = "measured"
        self.sync()
        with self.assertRaisesRegex(evaluate.ValidationError, "human source review"):
            self.score()

    def test_empty_rationale_and_false_source_correctness_are_rejected(self):
        self.adjudication()["findings"][0]["rationale"]["trigger"] = " "
        self.sync()
        with self.assertRaisesRegex(evaluate.ValidationError, "empty string"):
            self.score()
        self.adjudication()["findings"][0]["rationale"]["trigger"] = "first([10,20])"
        self.adjudication()["findings"][0]["explanation"]["sourceCorrectness"] = False
        self.sync()
        with self.assertRaisesRegex(evaluate.ValidationError, "source-correct"):
            self.score()

    def test_strict_json_duplicate_keys_nonfinite_numbers_and_unknown_fields(self):
        path = self.root / "bad.json"
        for content in ('{"a":1,"a":2}', '{"a":NaN}', '{"a":1e999}'):
            path.write_text(content)
            with self.assertRaises(evaluate.ValidationError):
                evaluate.load_json(path)
        self.run["extra"] = "unexpected"
        self.sync()
        with self.assertRaisesRegex(evaluate.ValidationError, "unknown fields"):
            self.score()

    def test_cli_malformed_oversized_integer_and_deep_json_exit_two_without_traceback(self):
        path = self.root / "parser-limit.json"
        for content in ('{"schemaVersion":' + "9" * 5000 + '}', "[" * 2000 + "0" + "]" * 2000):
            with self.subTest(kind="oversized integer" if content.startswith("{") else "deep nesting"):
                path.write_text(content)
                stdout, stderr = io.StringIO(), io.StringIO()
                with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                    status = evaluate.main(["validate", "--manifest", str(path)])
                self.assertEqual(status, 2)
                self.assertEqual(stdout.getvalue(), "")
                self.assertIn("validation error:", stderr.getvalue())
                self.assertNotIn("Traceback", stderr.getvalue())

    def test_load_json_wraps_native_parser_limits_but_preserves_validation_errors(self):
        path = self.root / "parser-limit.json"
        path.write_text("{}")
        for error in (ValueError("decoder integer limit"), RecursionError("decoder recursion limit")):
            with self.subTest(error=type(error).__name__):
                with mock.patch.object(evaluate.json, "loads", side_effect=error):
                    with self.assertRaises(evaluate.ValidationError) as raised:
                        evaluate.load_json(path)
                self.assertIn(str(path), str(raised.exception))
                self.assertIn(str(error), str(raised.exception))
        path.write_text('{"a": 1, "a": 2}')
        with self.assertRaises(evaluate.ValidationError) as raised:
            evaluate.load_json(path)
        self.assertEqual(str(raised.exception), "JSON: duplicate object key 'a'")

    def test_malformed_native_boolean_and_finding_types_are_rejected(self):
        original = self.report()
        variants = [dict(partial="false"), dict(findings={}), dict(budget=dict(deferred=True)), dict(matrix=[dict(file="adapter.py", correctness="0.1")])]
        for change in variants:
            report = copy.deepcopy(original)
            report.update(change)
            self.write(self.report_path(), report)
            self.sync()
            with self.assertRaises(evaluate.ValidationError):
                self.score()
        report = copy.deepcopy(original)
        report["findings"][0]["line"] = True
        self.write(self.report_path(), report)
        self.sync()
        with self.assertRaisesRegex(evaluate.ValidationError, "expected integer"):
            self.score()

    def test_extreme_integer_probability_is_validation_error_and_cli_exit_two(self):
        report = self.report()
        report["findings"][0]["probability"] = 10 ** 1000
        self.write(self.report_path(), report)
        self.sync()
        with self.assertRaisesRegex(evaluate.ValidationError, "number out of range"):
            self.score()
        # Bypass only the product minimum inventory for this tiny stored test
        # corpus. The CLI still executes the real parse, identity and scoring
        # validation path, and must fail cleanly without a result/traceback.
        original_validator = evaluate.validate_manifest
        stdout, stderr = io.StringIO(), io.StringIO()
        with mock.patch.object(evaluate, "validate_manifest", side_effect=lambda path, require_inventory=True: original_validator(path, False)):
            with contextlib.redirect_stdout(stdout), contextlib.redirect_stderr(stderr):
                status = evaluate.main(["score", "--manifest", str(self.manifest_path), "--run", str(self.run_path), "--adjudications", str(self.adj_path)])
        self.assertEqual(status, 2)
        self.assertEqual(stdout.getvalue(), "")
        self.assertIn("validation error:", stderr.getvalue())
        self.assertNotIn("Traceback", stderr.getvalue())

    def test_evidence_path_traversal_and_symlinks_are_rejected(self):
        self.entry()["report"] = "../outside.json"
        self.write(self.run_path, self.run)
        with self.assertRaisesRegex(evaluate.ValidationError, "unsafe relative path"):
            self.score()
        self.entry()["report"] = "reports/unit-defect.json"
        link = self.root / "reports" / "linked.json"
        link.symlink_to(self.root / self.entry()["report"])
        self.entry()["report"] = "reports/linked.json"
        self.write(self.run_path, self.run)
        with self.assertRaisesRegex(evaluate.ValidationError, "symbolic links"):
            self.score()

    def test_malformed_manifest_duplicate_case_and_outside_source_location(self):
        self.manifest["cases"].append(copy.deepcopy(self.manifest["cases"][0]))
        self.write(self.manifest_path, self.manifest)
        with self.assertRaisesRegex(evaluate.ValidationError, "duplicate case"):
            evaluate.validate_manifest(self.manifest_path, False)
        self.manifest["cases"].pop()
        self.manifest["cases"][0]["issues"][0]["endLine"] = 999
        self.write(self.manifest_path, self.manifest)
        with self.assertRaisesRegex(evaluate.ValidationError, "location outside source"):
            evaluate.validate_manifest(self.manifest_path, False)

    def test_rehashed_but_wrong_diff_cannot_replace_exact_head(self):
        case = self.manifest["cases"][0]
        diff = self.root / case["diff"]
        diff.write_text(diff.read_text().replace("+    return items[1]", "+    return items[2]"))
        case["source"]["diffSha256"] = evaluate.sha256(diff)
        self.write(self.manifest_path, self.manifest)
        with self.assertRaisesRegex(evaluate.ValidationError, "does not reproduce exact head"):
            evaluate.validate_manifest(self.manifest_path, False)

    def test_no_final_newline_diff_metadata_reproduces_head(self):
        case = self.manifest["cases"][0]
        head = self.root / case["head"]
        (head / "adapter.py").write_text("def first(items):\n    return items[1]")
        diff = self.root / case["diff"]
        diff.write_text("--- a/adapter.py\n+++ b/adapter.py\n@@ -1,2 +1,2 @@\n def first(items):\n-    return items[0]\n+    return items[1]\n\\ No newline at end of file\n")
        case["source"]["headSha256"] = evaluate.tree_sha256(head)
        case["source"]["diffSha256"] = evaluate.sha256(diff)
        self.write(self.manifest_path, self.manifest)
        evaluate.validate_manifest(self.manifest_path, False)

    def test_unsafe_diff_paths_and_symlink_modes_are_rejected(self):
        case = self.manifest["cases"][0]
        diff = self.root / case["diff"]
        for text in ("diff --git a/link b/link\nnew file mode 120000\n--- /dev/null\n+++ b/link\n@@ -0,0 +1 @@\n+/tmp/outside\n", "--- a/../../outside\n+++ b/../../outside\n@@ -0,0 +1 @@\n+outside\n"):
            diff.write_text(text)
            case["source"]["diffSha256"] = evaluate.sha256(diff)
            self.write(self.manifest_path, self.manifest)
            with self.assertRaises(evaluate.ValidationError):
                evaluate.validate_manifest(self.manifest_path, False)

    def test_unexpected_abstention_does_not_claim_complete_clean_case(self):
        self.entry("unit-fixed")["abstention"] = {"abstained": True, "rationale": "declined review"}
        self.sync()
        result = self.score()
        self.assertEqual(result["totals"]["unexpectedAbstentions"], 1)
        self.assertFalse(self.result_case("unit-fixed", result)["complete"])
        self.assertIsNone(result["rates"]["cleanCaseFalseAlarmRate"])

    def test_prepare_creates_only_pending_evidence_and_refuses_overwrite(self):
        output = self.root / "prepared"
        with contextlib.redirect_stdout(io.StringIO()):
            status = evaluate.main(["prepare", "--manifest", str(HERE / "manifest.json"), "--output", str(output)])
        self.assertEqual(status, 0)
        run = evaluate.load_json(output / "run.json")
        adjudications = evaluate.load_json(output / "adjudications.json")
        self.assertTrue(all(c["status"] == "missing" and c["report"] is None for c in run["cases"]))
        self.assertFalse(adjudications["reviewer"]["independent"])
        self.assertEqual(adjudications["cases"], [])
        original = (output / "run.json").read_bytes()
        with contextlib.redirect_stderr(io.StringIO()):
            status = evaluate.main(["prepare", "--manifest", str(HERE / "manifest.json"), "--output", str(output)])
        self.assertEqual(status, 2)
        self.assertEqual((output / "run.json").read_bytes(), original)


if __name__ == "__main__":
    unittest.main()
