# Dispatch queue
The dispatcher owns its mutable queue. drain must return every queued job once, in order, and leave the queue empty.
