# v0.3.0

This release adds target-specific CycloneDX SBOMs, signed SLSA provenance
and SBOM attestations for the final Linux/macOS downloads, and a release
manifest with toolchain, source, dependency and Apple signing evidence.
The macOS binary is Developer ID signed and Apple notarized.

Publication verifies a draft before making it immutable, checks exactly
17 release assets, and advances v0 only after validation of the newest
stable release. Backports/prereleases preserve the major tag.

The Action now fails closed for required verification, supports explicit
legacy checksum-only installs for older releases, and builds from source
only when requested. Consumers can verify source commit/ref and signer
identity before execution. L3 work is deferred; no L3 or certification
claim is made. See docs/slsa.md for trust requirements and limitations.
