# Review evidence contract

Source review identity is authoritative. Auxiliary evidence at another head, or without a captured source head, must preserve reviewedHead and invalidate completeness. Unknown evidence must prevent automatic approval. This is intentional hardening. Tests auxiliary_head_cannot_overwrite_verified_source_identity and auxiliary_head_cannot_fill_missing_source_identity demonstrate both boundaries.
