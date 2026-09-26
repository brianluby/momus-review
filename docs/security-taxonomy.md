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

- `securityCrypto` — weak/missing/rolled crypto + hardcoded secrets (#6, #7)
- `securityMisconfig` — misconfiguration breadth (#8)

Input-validation (#9), SSRF/open-redirect/CSRF/deserialization (#10–14) are
deferred: input validation is the biggest bucket but the fuzziest (prone to
"missing something I can't see" false positives); the others are lower volume.

Validation loop: re-run `momus-eval` after the steering change to confirm the
gained crypto/misconfig coverage does not regress injection/authz recall —
this is why the golden-set harness (B) precedes the taxonomy work.

## Validation — increment 1 (crypto/misconfig/secrets)

Re-scanned Juice Shop (`1618a611b`, same scope/excludes) after adding the two
`noul`s. Before → after:

| signal | before | after |
|---|---|---|
| findings (total) | 99 | 106 |
| security findings | 39 | 38 |
| **Cryptography** (A02) | **0** | **1** (`cryptographicFailure` + a hardcoded-secret fixture `sensitiveDataExposure`) |
| Security Misconfiguration (A05) | 0 | **0** |
| injection-family labels | 13 flat `injection` | 19 split (`sqlInjection` 2, `noSqlInjection` 3, `xss` 2, `xxe` 1, `ssrf` 1, `pathTraversal` 7) |

Read: crypto steering fires (gap partially closed), no injection/authz recall
regression, classification is now genuinely granular. **Misconfiguration
steering did not fire** — its `noul` is added but unvalidated; either the
question text is too weak or Juice Shop's misconfig defects aren't in the
scanned `.ts`/`.js` paths. Open follow-ups:

1. Tune/verify the `misconfig` question (or accept it needs a fixture-driven test).
2. Re-align the curated `known_vulnerable` table: its single-category-per-file
   entries no longer match the finer mechanism labels (e.g. `changePassword.ts`
   now surfaces `cryptographicFailure`, but the table expects only "Broken
   Authentication"), so category-matched corroboration drops on re-classified
   findings — a table precision issue, not a recall regression.