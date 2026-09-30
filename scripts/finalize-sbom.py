#!/usr/bin/env python3
"""Add a deterministic CycloneDX serial number required by actions/attest.

cargo-cyclonedx omits serialNumber when SOURCE_DATE_EPOCH is set. CycloneDX
allows that, but our pinned attestation action requires it to detect the
format. Derive a UUID from the canonical document without changing its graph.
"""

import argparse
import hashlib
import json
from pathlib import Path
import uuid


def finalize(bom):
    if not isinstance(bom, dict) or bom.get("bomFormat") != "CycloneDX":
        raise ValueError("expected a CycloneDX object")
    if bom.get("specVersion") != "1.5":
        raise ValueError("expected CycloneDX 1.5")
    if "serialNumber" in bom:
        serial = bom["serialNumber"]
        if not isinstance(serial, str) or not serial.startswith("urn:uuid:"):
            raise ValueError("invalid CycloneDX serialNumber")
        if str(uuid.UUID(serial[9:])) != serial[9:]:
            raise ValueError("invalid CycloneDX serialNumber")
        return bom
    canonical = json.dumps(bom, sort_keys=True, separators=(",", ":"))
    digest = hashlib.sha256(canonical.encode()).hexdigest()
    result = dict(bom)
    result["serialNumber"] = "urn:uuid:" + str(uuid.uuid5(
        uuid.NAMESPACE_URL, "https://github.com/brianluby/momus-review/sbom/" + digest
    ))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sbom", required=True, type=Path)
    args = parser.parse_args()
    try:
        bom = finalize(json.loads(args.sbom.read_text()))
    except (OSError, ValueError) as exc:
        parser.exit(1, f"error: {exc}\n")
    args.sbom.write_text(json.dumps(bom, sort_keys=True, indent=2) + "\n")


if __name__ == "__main__":
    main()
