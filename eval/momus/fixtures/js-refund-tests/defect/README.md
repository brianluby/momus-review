# Refund ledger
Refund requests carry an idempotency key. The first key credits balance and returns true; repeating it returns false without a second credit. Tests for this new public operation must exercise the repeat-key branch.
