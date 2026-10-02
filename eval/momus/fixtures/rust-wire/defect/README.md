# Worker health endpoint
The stable JSON response key is state; existing clients search for state=ready. Tracing may add a trace key while retaining state. Trace identifiers are generated internally using only lowercase ASCII letters, digits and hyphens.
