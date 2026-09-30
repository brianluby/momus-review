#!/usr/bin/env python3
"""Exact release inventory and stable/prerelease tag policy."""
import argparse
import json
from pathlib import Path
import re

TARGETS = ('x86_64-unknown-linux-gnu', 'aarch64-unknown-linux-gnu', 'aarch64-apple-darwin')

def assets():
    return {'release-manifest.json', 'release-manifest.json.sha256'} | {
        f'momus-{t}.{suffix}' for t in TARGETS for suffix in
        ('tar.gz', 'tar.gz.sha256', 'cdx.json', 'provenance.bundle.json', 'sbom-attestation.bundle.json')}

def version(tag, releases):
    match = re.fullmatch(r'v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z.-]+))?', tag)
    if not match:
        raise ValueError(f'invalid release tag: {tag}')
    current = tuple(int(x) for x in match.groups()[:3])
    stable = []
    # gh --paginate --slurp returns pages; flat fixtures are also accepted.
    entries = [release for page in releases for release in (page if isinstance(page, list) else [page])]
    for release in entries:
        if release.get('draft') or release.get('prerelease'):
            continue
        m = re.fullmatch(r'v(\d+)\.(\d+)\.(\d+)', release['tag_name'])
        if m:
            stable.append(tuple(int(x) for x in m.groups()))
    prerelease = match[4] is not None
    return {'prerelease': prerelease, 'latest': not prerelease and current == max(stable + [current]),
            'advance_major': not prerelease and current == max([v for v in stable if v[0] == current[0]] + [current]),
            'major': f'v{current[0]}'}

def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    p = sub.add_parser('version'); p.add_argument('--tag', required=True); p.add_argument('--releases', required=True, type=Path)
    p = sub.add_parser('inventory'); p.add_argument('--dir', required=True, type=Path); p.add_argument('--fragments', action='store_true')
    p = sub.add_parser('published'); p.add_argument('--json', required=True, type=Path)
    args = parser.parse_args()
    try:
        if args.command == 'version':
            print(json.dumps(version(args.tag, json.loads(args.releases.read_text())))); return
        expected = assets()
        if args.command == 'inventory':
            if args.fragments: expected |= {f'momus-{t}.fragment.json' for t in TARGETS}
            paths = list(args.dir.iterdir()); actual = {p.name for p in paths}
            if any(not p.is_file() or p.is_symlink() for p in paths): raise ValueError('inventory contains nonregular files')
        else:
            release = json.loads(args.json.read_text())
            if release.get('immutable') is not True or release.get('draft') is not False:
                raise ValueError('published release must be immutable and not a draft')
            names = [p['name'] for p in release['assets']]
            if len(names) != len(set(names)): raise ValueError('duplicate published assets')
            actual = set(names)
        if actual != expected:
            raise ValueError(f'missing assets: {sorted(expected-actual)}; unexpected assets: {sorted(actual-expected)}')
        print(f'asset inventory ok: {len(expected)} files')
    except (ValueError, OSError, KeyError) as exc:
        parser.exit(1, f'error: {exc}\n')

if __name__ == '__main__': main()
