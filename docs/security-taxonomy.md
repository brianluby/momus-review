# Security screening taxonomy

What the `security` screen *steers toward* today vs. the full code-findable
security space. Grounded in OWASP categories and Juice Shop's challenge counts
(`data/static/challenges.yml`), from the golden-set eval (`eval/README.md`).

## The screen is a bounded net

Every screen question is one `noul` (yes/no) with a single `focus` list. The
security question's focus is:

> Authorization, injection, secret exposure, trust boundaries, and unsafe defaults

That's the whole net. Anything not in those five terms is never flagged, no
matter how comprehensive the mechanism (locate) vocabulary is — a signal must
clear the screen before the mechanism step runs.

## Full enumeration (security dimension)

| # | concern class | OWASP | Juice Shop challenges | static-findable | status |
|---|---|---|---|---|---|
| 1 | access control / privilege / IDOR | A01 | 12 | ✓ | **covered** ("authorization") |
| 2 | injection (SQL/NoSQL/cmd/SSTI/XSS/XXE) | A03 + XSS/XXE | 14 + 9 + 2 | ✓ | **covered** ("injection", coarse) |
| 3 | secret/credential exposure | 2017 A3 | (in SDE) | ✓ | **covered** ("secret exposure") |
| 4 | trust-boundary weakness | cross-cutting | — | ✓ | **covered** ("trust boundaries") |
| 5 | unsafe defaults | A05 (narrow) | 5 | ✓ | **covered** (produced 0) |
| 6 | **cryptography** (weak/missing/rolled-own, ECB, hardcoded IV, bad RNG, plaintext secrets) | A02 | **5** | ✓ | **gap** |
| 7 | **hardcoded secrets / keys in code** | A02 / SDE | (some) | ✓ | **gap** |
| 8 | **security misconfiguration breadth** (missing headers, permissive CORS, debug, dir listing, verbose errors, default creds) | A05 | **5** | ✓ | **gap** |
| 9 | **improper input validation / mass assignment** | cross-cutting | **12** | ✓ (fuzzy) | **gap** |
| 10 | insecure deserialization | 2017 A8 | 3 | ✓ | partial (injection) |
| 11 | SSRF | A10 | (under BAC) | ✓ | partial (injection) |
| 12 | open redirect / unvalidated forwards | A10-ish | 2 | ✓ | gap |
| 13 | CSRF | cross-cutting | (none) | ✓ | gap |
| 14 | TOCTOU / check-then-use (auth→use, open→read) | overlaps reliability | — | ✓ | partial (reliability) |

## Deliberately out of scope

| concern | why |
|---|---|
| Vulnerable/outdated components (9) | dependency/SCA scanning, not diff review |
| Broken anti-automation (4) | runtime (rate-limit/CAPTCHA) |
| Security through obscurity (3), Miscellaneous (6) | not static-code-findable |

## Structure (how to broaden without diluting)

The `focus` is one string on one `noul`. Enumerating everything into it
dilutes model attention and risks regressing the covered classes. Three shapes:

- **A** — broaden the one `focus` + true/false `examples` in place (cheapest, highest dilution).
- **B** — split security screening into 2–3 `noul`s (one per concern family),
  batched in the same `system_one` call, folded back into the `security`
  dimension probability by **max-merge** (the codebase mode already max-merges
  per region). No report-schema change.
- **C** — steer only the locate `choice` (Lever 1), leave screening coarse.

## Decision (2026-09-25)

**Option B**, first increment scoped to the high-precision, low-noise classes:

- `cryptoSecrets` — weak/missing/rolled crypto + hardcoded secrets (#6, #7)
- `misconfig` — misconfiguration breadth (#8)

Input-validation (#9), SSRF/open-redirect/CSRF/deserialization (#10–14) are
deferred: input validation is the biggest bucket but the fuzziest (prone to
"missing something I can't see" false positives); the others are lower volume.

Validation loop: re-run `momus-eval` after the steering change to confirm the
gained crypto/misconfig coverage does not regress injection/authz recall —
this is why the golden-set harness (B) precedes the taxonomy work.

## Validation — increment 1 (crypto/misconfig/secrets)

Re-scanned Juice Shop (`1618a611b`, same scope/excludes) after adding the two
`noul`s, plus fixture checks:

| signal | before | after |
|---|---|---|
| findings (total) | 99 | 106 |
| security findings | 39 | 38 |
| **Cryptography** (A02) | **0** | **1** |
| Security Misconfiguration (A05) | 0 | 0 (juice-shop scan) / **fires** on a pure-misconfig fixture |
| injection-family labels | 13 flat `injection` | 16 granular (`sqlInjection` 2, `noSqlInjection` 3, `xss` 2, `xxe` 1, `ssrf` 1, `pathTraversal` 7) |

Read:

- Crypto steering fires (gap partially closed); no injection/authz recall regression.
- **Misconfiguration steering is verified working** — a pure-misconfig fixture
  (CORS `*`, `dotfiles` directory listing, verbose stack) yields a
  `securityMisconfiguration` finding. The 0 in the juice-shop scan is
  classification overlap, not a broken question: e.g. a `/debug` endpoint that
  dumps `process.env` is correctly labeled `sensitiveDataExposure`, and Juice
  Shop's own misconfig defects are subtle or sit in excluded paths.
- Corroboration is now **file-level** (`momus-eval`): a security finding on any
  curated `known_vulnerable` route counts regardless of its (finer) category
  label; category agreement is reported as a secondary diagnostic, no longer a
  gate. The curated table's one-category-per-file granularity no longer
  suppresses the score.
## Per-language security classes (2026-09-26, ticket #7)

Discovery widened to the 20-language consensus set, and each language
contributes its own classification mechanisms (`domain/policy.rs`
`LANGUAGE_MECHANISMS`) — the security screen's focus is unchanged; the
per-language rows are what `locate` may classify a located signal as. The
security additions, by class:

| mechanism | languages | OWASP / CWE | evaluator category |
|---|---|---|---|
| `dynamicCodeExecution` | JS/TS, Python, Ruby, shell, PowerShell | A03 injection (CWE-95) | Injection |
| `prototypePollution` | JS/TS | A03 / CWE-1321 | Injection |
| `formatString` | C | A03 / CWE-134 | Injection |
| `unsafeReflection` | Java | A03 / CWE-470 | Injection |
| `unquotedExpansion` | shell, PowerShell | A03 / CWE-78 (argument injection) | Injection |
| `assertForValidation` | Python | CWE-617 (validation removed by `-O`) | Improper Input Validation |
| `fileInclusion` | PHP | A01 / CWE-98 (LFI/RFI) | Broken Access Control |
| `massAssignment` | PHP, Ruby | CWE-915 | Broken Access Control |
| `unescapedTemplate` | Go | A03 / CWE-79 | XSS |
| `overbroadGrant` | SQL | A05 / CWE-732 | Security Misconfiguration |
| `bufferOverflow` | C | CWE-120 | Miscellaneous |
| `useAfterFree` | C, C++ | CWE-416 | Miscellaneous |
| `outOfBoundsAccess` | C++ | CWE-125/787 | Miscellaneous |

Two notes a reader should not have to reverse-engineer:

- **Memory-safety classes map to `Miscellaneous`.** The golden-set evaluator's
  categories come from Juice Shop's OWASP list, which has no memory-safety
  class (the benchmark is JS/TS). Mapping them to a category keeps the
  evaluator counting them instead of silently dropping them; it does not make
  them corroborable against that benchmark.
- **Rust's `unsafe` lives in correctness, not security.** `unsafeBlock` is
  about the invariant the block assumes, which the invariant vocabulary
  expresses better than the security classes do. The classifier can still
  reach any generic security mechanism (`commandInjection`, …) for Rust.

`bin/momus-eval.rs::every_security_mechanism_maps_to_a_category` iterates the
generic vocabulary plus all 20 languages, so a new security key without a
category mapping fails the suite rather than skewing a score.
