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
2. **File-level precision** (curated): match findings against a
   `known_vulnerable` table of `file → categories`. Not yet populated — the
   mapping from challenge `key` to code location is not machine-readable in
   Juice Shop, so it must be curated by hand rather than guessed.

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