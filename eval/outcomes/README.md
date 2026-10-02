# Outcome evidence collector

Read [the collection protocol](../../docs/outcome-collection.md) before using
`collect.py`. It freezes existing committed premerge scores, validates trusted
source receipts, joins exact merged PR heads, and exports independent observed
outcomes with a separate label-availability audit. Missing telemetry stays
unknown; no model inference or real calibration campaign runs here.

Run the deterministic, fabricated-input mechanics tests:

```sh
rtk proxy python3 -m unittest discover -s eval/outcomes -p 'test_*.py'
```

Store real protocol/capture/merge/telemetry files in maintainer-controlled
evidence storage outside the checkout. No real history or measured results are
included in this directory. Collection implementation child #60 can complete
independently; merge-confidence/outcome-evidence parents #17/#51 still require
the real external evidence described in the protocol.
