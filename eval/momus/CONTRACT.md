# Offline product benchmark contract v1

The corpus and scorer are independent of live inference. Authored fixtures are
evaluation examples, not observed production defects. Dataset v1 is never used
for prompt tuning or Jeeves training. PR #40 examples are development controls.

`manifest.json` contains `schemaVersion: 1`, `datasetVersion: "momus-owned-v1"`,
`cases`. Each case has `id`, `pairId` (null for unpaired cases), `split`
(`holdout` or `development`), `slice` (`core` or `auxiliary`), `language`
(`rust`, `javascript`, `python`, `docs`, `dependency`), `expectation`
(`defect`, `clean`, `abstain`), `dimension` (Momus dimension, `docs` or
`dependency`), `base`, `head`, `diff` (paths relative to this directory),
`source` (kind, license, description and exact content hashes), `issues`,
`abstention` (null or concrete missing-evidence rationale), `reproducer`
(command argv or null), and `annotation` (review status and rationale).

Each issue has `id`, `file`, `startLine`, `endLine`, `dimension`, `trigger`,
`contract`, `consequence`, `acceptableFinding`, `explanation` and `fix`.
Clean/abstention cases have no issues. Content tree hashes use SHA-256 over
sorted relative file paths and each file's SHA-256 via canonical JSON;
diff hashes use SHA-256 over exact diff bytes. No timestamps enter identities.
The corpus lane will finalize the precise `source` hash keys and tell the
scorer lane before integration. Corpus integrity, minimum inventory,
nonempty diffs and source locations are validated independently.

Run receipts name every case, report path and exact SHA-256, and completion
status. Stored Momus reports retain their native findings shape. Separate
adjudications bind each finding index to the report hash and issue id, with
source-grounded rationale; same-file/dimension/line matches alone never earn
credit. Evidence must establish the trigger, contract and consequence.
Explanation assessment separately records source correctness, specificity,
actionability and uncertainty. Labels and adjudications must never be given to
the model. Partial/missing/failed cases cannot earn detection credit and must
remain in denominators. Duplicate findings earn at most one detection and are
reported explicitly. Unrelated findings are false alarms even in defect cases.

Executable entry points are Python standard-library scripts in this directory:
`evaluate.py validate`, `evaluate.py score`, `materialize.py` for sanitized
base/head Git workspaces, and `reproduce.py` for offline behavioral controls.
Only the coordinating agent changes Git, tracker, CI and top-level docs.
