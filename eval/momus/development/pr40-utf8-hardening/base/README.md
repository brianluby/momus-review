# Review evidence contract

Git source evidence must remain exact UTF-8. Invalid bytes must fail explicitly; lossy replacement characters cannot prove a source fact. Diagnostics may decode stderr lossily. Rejecting invalid source bytes is an intentional evidence-integrity contract. See the unreadable_source_baseline_is_never_reinterpreted_as_an_addition regression test.
