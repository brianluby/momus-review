# v0.3.1

Fix required attestation verification on macOS system Bash 3.2 when optional signer-digest or source-ref pins are omitted. The verifier keeps its required source policy in a nonempty array; optional policy flags are still omitted unless supplied.

Add a macOS system-Bash CI gate covering installer success, explicit legacy/source compatibility, rejection before execution, optional verification pins, and tampered release assets. Release archives, target SBOMs, attestation bundles and draft publication retain the v0.3.0 verification policy.
