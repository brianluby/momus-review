# PR #40 local review dispositions

OCR candidate numbers below follow the final ordered 84-comment result from
session `1c08a612-d34f-44bc-9d5d-a47a1365db93`, reviewing first implementation
commit `7a6b4690e1802a50bd2b29e7da3eb2073eb14fa4`. Duplicates are grouped.
CodeRabbit and Copilot inline dispositions are recorded separately on PR #40.
No real calibration or second OCR run is implied by this record.

| Candidates | Disposition and evidence |
| --- | --- |
| 1–11 | Dashboard builders now precede rendering, label actual serialized evidence, scopes, changes, outcome status, provenance, windows and uncertainty, disclose approval policy/identity, retain unknowns, bound lists/text and display the heuristic as a score. Eleven Node regressions run in CI. |
| 12–17 | Action tests use runner Bash semantics, verify declared environment/input mappings, independent flag combinations, separate stub/output directories and head mismatches. Eight cases also run under macOS system Bash. Extraction failures are explicit. |
| 18–25, 32 | GitHub summaries use plain versions, bounded inert/redacted text, omission notices, uncertainty bounds and early identity-bound decisions. Regressions cover stale eligibility, synthetic provenance, hostile Markdown, list overflow and the 64,000-byte body limit. |
| 26, 28 | Verification booleans retain backward-compatible serialization: false means verification is unavailable, not proven dirtiness. Labels and field documentation now state that distinction. An assessment artifact can exist with unknown outcomes; its presence does not imply calibration or policy opt-in. |
| 27, 41, 56 | Auxiliary analysis preserves source-review identity; mismatched or missing identities clear verification, mark the report partial and record an unknown. Unit fixtures exercise both cases. |
| 29–30 | Baseline states and historical rename attribution are documented. Retained-byte accounting now includes both copies of changed current text, with boundary regressions. It does not claim to bound total process memory. |
| 31, 36–38 | Histories require the heuristic producer version and canonical nonsynthetic Git identities; all check-evidence assertions are required. Approval checks actual usable training/held-out observation and cohort recency, not only export headers. Fixtures cover aliases, omitted fields, incompatible versions and fresh exports containing old telemetry. |
| 33–34 | Retained compatibility/trust boundary: saved artifacts are assessments, not authenticated authorization. Actual approval requires an explicit maintainer-controlled policy and fresh live reassessment. Unknown/noncanonical status strings reject eligibility. Neither stored decisions nor free-string statuses authorize an API approval by themselves. |
| 35, 44–45, 51–53 | Corrected module/function documentation, CLI help, reciprocal policy/history arguments, input-path diagnostics and committed/auxiliary incompatibility guidance. |
| 39 | The universal-failure claim was overstated: this repository ignores `/reviews`; fixture checkouts must specify their own ignored or external output paths. The trusted setup documents those paths. Retained conservative post-run cleanliness verification rather than excluding arbitrary tool paths from integrity checks. |
| 40, 47, 80 | Missing heads cannot assert cleanliness. Committed scopes must share one canonical checkout; public pinned reads validate full commit identities and all scopes. Empty scopes are checked before indexing. |
| 42–43, 50 | Approval rejection distinguishes GitHub review rejection from policy rejection. Final publication failure preserves the completed source review and sanitized artifact with eligibility cleared and an explicit unconfirmed-publication reason. Duplicate writes occur only when reassessment changes the artifact. |
| 46, 49 | Tour documentation now discloses index-cache writes and working-tree provenance. Dirty/untracked evidence produces an explicit unknown; the head is a baseline, not a claim that tour text is committed. |
| 48 | Failed default outcome assessment no longer loses a finished source review: retain it as partial, leave the assessment unavailable and emit a diagnostic. Invalid severity has a CLI regression. |
| 54–55 | CLI tests require actual outcome arrays, all three present null probabilities, enabled policy and the synthetic-rejection reason. Missing JSON fields cannot pass vacuously. |
| 57, 59 | Local redaction now adds an explicit unknown when credential values cannot be compared. Outgoing API redaction counters retain their original meaning; local duplicate evidence scrubs do not inflate those counters. Secret rotation and finding-order regressions cover this path. |
| 58 | Only appended advisory findings are sorted. Existing pairwise/refinement order is preserved by regression. |
| 60 | Findings intentionally join the canonical top-level report list for fingerprints, suppression and publication; nested summary lists are internal handoffs. Feature guides specify top-level ownership, avoiding duplicated actionable findings. |
| 61, 73–76 | Committed context has explicit fail-before-screening completeness semantics. Unreadable source baselines cannot masquerade as additions. Full pinned identities are validated; error text names the actual pinned head. A bounded batched Git blob read replaces per-file processes, with 1,600-file and over-limit real-Git fixtures. |
| 62–63 | Docs analysis lexes each source once, indexes exact identifiers and opaque exports, and precomputes example shadows. Unrelated unsupported declarations do not create unknowns unless linked to documentation. Repeated calls, comment/string identifiers and unsupported exports have regressions. |
| 64–66, 81–84 | Reusable exact import indexes and tour path indexes replace quadratic scans. Dotted names are preserved; unbalanced opaque syntax discards a whole file's edges. Index fallback counts become unknown declaration evidence. Full-line comments were already skipped; whole-file opaque guards remain deliberately conservative and documented. |
| 67–68 | Citation lookup is optional and ambiguity-preserving, with unlocated evidence/unknowns rather than invented line one. Escaped/duplicate JSON and unusual TOML decoder fixtures exercise attribution limits. Parsed results are moved instead of deeply cloned. |
| 69 | Retained advisory semantics: release-note wording such as “removed” is a review signal, not a proven public API break. Exact dependency/release identity, cited text and usage/intervening-release uncertainty remain visible; findings do not request changes from release notes alone. |
| 70 | The CLI owns auxiliary attachment after source identity binding. Direct `run_review` callers with unconsumed auxiliary options now receive an explicit error instead of silently losing their requested analysis; field documentation states the ownership boundary. |
| 71 | Provider check states retain failed/pending/unknown distinctions, and unnamed checks are explicitly labeled. Invalid timestamp zero remains a fail-closed sentinel. |
| 72 | The all-runs premise contradicted GitHub's documented default `filter=latest`. The request now states `latest` explicitly; duplicate ambiguous names and truncated inventories still reject approval. See [GitHub check-runs API](https://docs.github.com/en/rest/checks/runs?apiVersion=2026-03-10). |
| 77–79 | Untracked membership is collected once per scope; helper documentation distinguishes tracked, untracked and ignored files. Unreadable auxiliary base blobs become explicit unknowns while other readable evidence survives. |

Retained limitations are deliberate abstentions or compatibility boundaries,
not claims of complete semantic analysis, authenticated telemetry or real-world
calibration. Prospective outcome collection remains Veans #51 under open #17.
