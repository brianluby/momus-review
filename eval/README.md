# Golden-set evaluation

Measures how a `momus` report fares against a known-vulnerable target.
Currently scoped to **Juice Shop** (OWASP's intentionally-vulnerable app).

## What it measures

Two tiers, deliberately scoped to *security* findings (Juice Shop is a
security benchmark; its 116 challenges have no ground truth for correctness/
reliability/testGap findings):

1. **Category coverage** (automated): map each security finding's *mechanism*
   to the OWASP categories it can indicate, and report which of the target's
   16 categories are touched.
2. **Curated corroboration rate** (curated, lower bound): match findings
   against a `known_vulnerable` table of `file → categories`. The table is
   populated (23 routes) but incomplete — the mapping from challenge `key` to
   code location is not machine-readable in Juice Shop, so it is curated by
   hand rather than derived, and uncurated routes are not counted as false
   positives.

## Run

```bash
momus scan . --exclude ... > report.json     # produce a report
momus-eval report.json eval/juice-shop/ground-truth.json
```

## Caveats

- `recall@116` is **not** the goal: roughly half of Juice Shop's challenges
  are runtime exploit-flags (bypass CAPTCHA, "register as admin") that a
  static review tool should *not* report as findings.
- The `mechanism → category` mapping in `src/bin/momus-eval.rs` is loose by
  design (e.g. `injection` indicates Injection/XSS/XXE/Deserialization); it
  is a coverage signal, not a classifier.
- Category counts in `ground-truth.json` are extracted from Juice Shop's
  `data/static/challenges.yml`.

## Current result (2026-09-25, Juice Shop @ `1618a611b`)

Scan scope: 299 files, 3 vendored `--exclude`s, 99 findings (39 security).

| measure | value |
|---|---|
| category coverage | **9/16** |
| recall over curated known-vulnerable routes | **19/23** (83%) |
| security findings on a known-vulnerable route | **19/39** (49%) |
| …with a matching category label | 11/39 (28%) |

The three findings this surfaces:

1. **Real coverage gaps** — Improper Input Validation (12), Security
   Misconfiguration (5), Cryptographic Issues (5) have no clean slot in the
   five-dimension / per-dimension-mechanism vocabulary.
2. **Good recall** — the tool hits 19 of 23 curated vulnerable routes.
3. **Taxonomy over-collapse** — the 8 category "mismatches" are on the *right
   file* but labeled differently (`injection` subsumes SSRF/XXE/file-upload and
   what Juice Shop calls "improper input validation" or "XXE"). Not wrong-file
   false positives, but a signal the `injection` mechanism is too coarse.

The 20 security findings on un-curated routes are not counted as false
positives — the `known_vulnerable` list is a conservative subset, not the full
challenge map.