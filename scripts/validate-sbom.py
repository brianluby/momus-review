#!/usr/bin/env python3
"""Validate a cargo-cyclonedx CycloneDX 1.5 SBOM for a momus release target.

Two layers:

1. Schema validation against a pinned CycloneDX specification schema
   (downloaded at a pinned commit by the caller, see release.yml). The
   schema directory must contain bom-1.5.schema.json plus its relative
   references (spdx.schema.json, jsf-0.82.schema.json).

2. Content checks the schema cannot express:
   - metadata.component describes this package (momus) at this version,
   - a non-empty resolved dependency graph with relationship entries,
   - every component carries a unique bom-ref identifier,
   - license information is present where Cargo metadata carries it
     (cargo-cyclonedx emits it; a large drop means generation is broken),
   - development-only dependencies are absent,
   - expected platform-specific crates are present / absent so the SBOM
     really is target-specific and not the all-platform union.

Exits 0 when every check passes; prints each failure otherwise.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

try:
    from jsonschema import Draft7Validator
except ImportError:  # pragma: no cover
    print("error: the jsonschema package is required (pip install jsonschema==4.25.0)")
    sys.exit(2)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--sbom", required=True, type=Path, help="CycloneDX JSON file to validate")
    parser.add_argument("--schema", required=True, type=Path, help="bom-1.5.schema.json path")
    parser.add_argument("--package", default="momus", help="expected top-level component name")
    parser.add_argument("--version", required=True, help="expected top-level component version")
    parser.add_argument("--expect-crate", action="append", default=[],
                        help="crate name that must appear in components (repeatable)")
    parser.add_argument("--forbid-crate", action="append", default=[],
                        help="crate name that must NOT appear in components (repeatable)")
    args = parser.parse_args()

    failures: list[str] = []

    try:
        bom = json.loads(args.sbom.read_text())
    except (OSError, json.JSONDecodeError) as exc:
        print(f"error: cannot read SBOM {args.sbom}: {exc}")
        return 1
    schema = json.loads(args.schema.read_text())

    # 1. Schema validation (fail fast on structural garbage).
    validator = Draft7Validator(schema)
    errors = sorted(validator.iter_errors(bom), key=lambda e: list(e.absolute_path))
    for error in errors:
        failures.append(f"schema violation at {'/'.join(map(str, error.absolute_path)) or '<root>'}: "
                        f"{error.message}")
    if errors:
        # Structural damage makes the content checks meaningless.
        for failure in failures:
            print(f"FAIL {failure}")
        return 1

    if bom.get("specVersion") != "1.5":
        failures.append(f"specVersion is {bom.get('specVersion')!r}, expected '1.5'")

    # 2. Top-level component identity.
    component = bom.get("metadata", {}).get("component", {})
    if component.get("name") != args.package:
        failures.append(f"metadata.component.name is {component.get('name')!r}, "
                        f"expected {args.package!r}")
    if component.get("version") != args.version:
        failures.append(f"metadata.component.version is {component.get('version')!r}, "
                        f"expected {args.version!r}")
    if component.get("type") != "application":
        failures.append(f"metadata.component.type is {component.get('type')!r}, "
                        "expected 'application'")

    # 3. Non-empty resolved graph with identifiers and relationships.
    components = bom.get("components") or []
    if not components:
        failures.append("components is empty: the resolved dependency graph is missing")
    bom_refs = [c.get("bom-ref") for c in components]
    if any(not ref for ref in bom_refs):
        failures.append("a component has no bom-ref identifier")
    if len(set(bom_refs)) != len(bom_refs):
        failures.append("duplicate bom-ref identifiers in components")
    dependencies = bom.get("dependencies") or []
    if not dependencies:
        failures.append("dependencies is empty: dependency relationships are missing")
    root_ref = component.get("bom-ref")
    if root_ref and not any(d.get("ref") == root_ref for d in dependencies):
        failures.append("no dependency relationship entry for the top-level component")

    # 4. License coverage. cargo-cyclonedx copies the license expression from
    # Cargo metadata; nearly every crates.io crate has one. A big drop means
    # generation ran against incomplete metadata.
    licensed = sum(1 for c in components if c.get("licenses"))
    if components and licensed * 2 < len(components):
        failures.append(f"only {licensed}/{len(components)} components carry license information")

    # 5. Target specificity and dev-dependency exclusion.
    names = {c.get("name") for c in components}
    for crate in args.expect_crate:
        if crate not in names:
            failures.append(f"expected platform crate {crate!r} is missing — "
                            "the SBOM does not describe this target's resolved graph")
    for crate in args.forbid_crate:
        if crate in names:
            failures.append(f"forbidden crate {crate!r} is present")

    if failures:
        for failure in failures:
            print(f"FAIL {failure}")
        return 1

    print(f"ok {args.sbom.name}: schema-valid CycloneDX 1.5, "
          f"{len(components)} components, {len(dependencies)} relationships, "
          f"{licensed} licensed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
