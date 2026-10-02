# Retry scheduler
Persisted attempt counts may be any u32. delay_ms must return a delay at most 10000 milliseconds without panicking, including restored counters above 63.
