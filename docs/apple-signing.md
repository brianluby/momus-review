# macOS signing and notarization

macOS release binaries are built and tested without signing credentials. The release workflow then calls the SHA-pinned shared workflow in [brianluby/apple-signing](https://github.com/brianluby/apple-signing) to sign with Developer ID, enable hardened runtime, obtain a trusted timestamp, and require Apple notarization acceptance. Publication depends on successful signing. The tar.gz and SHA-256 checksum are regenerated from the signed executable. Re-signing an accepted executable invalidates its notarization ticket.

## Credentials and approvals

The signing job uses this repository's `release-signing` environment. It requires a maintainer approval and is restricted to `main` and release tags (`v*`). Keep all four Apple secrets in that environment: `APPLE_CERTIFICATE_P12_BASE64`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_ID`, and `APPLE_APP_SPECIFIC_PASSWORD`. The certificate export must include its private key. Never put credentials in repository files or workflow inputs. See the shared repository README and private terminal setup helper for export/upload instructions.

## Rehearsal

Run `release` via Actions → Run workflow on **main** and approve `release-signing`. This signs/notarizes the macOS artifact and saves Apple evidence without publishing a release or moving tags. Verify the `apple-notarization-momus-aarch64-apple-darwin` evidence artifact and download the signed `momus-aarch64-apple-darwin` archive. On a Mac, extract a fresh copy and run `momus --help`. Verify with `codesign --verify --strict --check-notarization -R=notarized ./momus`.

Standalone Mach-O command-line tools cannot be stapled; macOS retrieves their tickets online. Preserve the notarized bytes and signature in every distribution archive. The existing Linux release artifacts are unchanged.
