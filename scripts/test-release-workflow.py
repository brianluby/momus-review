#!/usr/bin/env python3
"""Run actual publication workflow commands against a non-publishing gh double."""
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import textwrap
import unittest

ROOT=Path(__file__).resolve().parent.parent
WORKFLOW=(ROOT/'.github/workflows/release.yml').read_text()
STEPS=['Check the asset inventory','Require immutable releases','Refuse to overwrite an existing release',
       'Select release version policy','Create the draft release with the explicit asset set',
       'Verify the draft release as a consumer','Publish','Confirm the published asset set','Move the major tag']
spec=importlib.util.spec_from_file_location('release_policy',ROOT/'scripts/release-policy.py')
policy=importlib.util.module_from_spec(spec); spec.loader.exec_module(policy)

def block(name):
    match=re.search(r'      - name: '+re.escape(name)+r'\n.*?        run: \|\n((?:          [^\n]*\n|\n)+)',WORKFLOW,re.S)
    if not match: raise ValueError(name)
    return textwrap.dedent(match[1])

GH=r'''#!/usr/bin/env python3
import json,os,pathlib,shutil,sys
args=sys.argv[1:]; base=pathlib.Path(os.environ['STATE_DIR'])
with open(base/'calls','a') as f: f.write(json.dumps(args)+'\n')
if args[0]=='api':
    endpoint=next(a for a in args[1:] if a.startswith('repos/'))
    if endpoint.endswith('immutable-releases'):
        if os.environ.get('SETTING_ERROR'): print('HTTP '+os.environ['SETTING_ERROR'],file=sys.stderr); sys.exit(1)
        print(json.dumps({'enabled':os.environ.get('IMMUTABLE','true')=='true'}))
    elif '/releases/tags/' in endpoint:
        if (base/'published').exists():
            names=json.loads((base/'assets').read_text())
            if os.environ.get('BAD_PUBLISHED'): names=names[:-1]
            print(json.dumps({'draft':False,'immutable':os.environ.get('PUBLISHED_MUTABLE')!='1','assets':[{'name':n} for n in names]}))
        elif os.environ.get('EXISTS'): print('{}')
        else: print('HTTP '+os.environ.get('LOOKUP_ERROR','404'),file=sys.stderr); sys.exit(1)
    elif '/matching-refs/' in endpoint:
        print(json.dumps([{'ref':'refs/tags/'+t} for t in json.loads(os.environ['TAGS'])]))
    elif '/git/ref/tags/' in endpoint: print('{}')
    elif '/git/refs/' in endpoint: print('{}')
    else: sys.exit(9)
elif args[:2]==['release','create']:
    names=[pathlib.Path(a).name for a in args if a.startswith('dist/')]
    assert len(names)==17 and all(pathlib.Path('dist',n).is_file() for n in names)
    (base/'assets').write_text(json.dumps(names)); (base/'draft').touch()
elif args[:2]==['release','download']:
    dest=pathlib.Path(args[args.index('--dir')+1]); dest.mkdir(parents=True,exist_ok=True)
    for name in json.loads((base/'assets').read_text()): shutil.copyfile(pathlib.Path('dist',name),dest/name)
elif args[:2]==['release','edit']:
    assert (base/'verified').exists(); (base/'published').touch()
else: sys.exit(8)
'''
VERIFY='''#!/bin/bash
printf '%s\\n' "$*" >> "$STATE_DIR/verify-args"
[ "${VERIFY_FAIL:-0}" != 1 ] || exit 1
touch "$STATE_DIR/verified"
'''

class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.base=Path(self.tmp.name); (self.base/'dist').mkdir(); (self.base/'scripts').mkdir(); (self.base/'bin').mkdir()
        for name in policy.assets()|{f'momus-{t}.fragment.json' for t in policy.TARGETS}:
            (self.base/'dist'/name).write_text('fixture')
        (self.base/'scripts/release-policy.py').write_text((ROOT/'scripts/release-policy.py').read_text())
        p=self.base/'scripts/verify-release.sh'; p.write_text(VERIFY); p.chmod(0o755)
        p=self.base/'bin/gh'; p.write_text(GH); p.chmod(0o755)
        self.env={**os.environ,'PATH':str(self.base/'bin')+os.pathsep+os.environ['PATH'],
            'RUNNER_TEMP':str(self.base),'STATE_DIR':str(self.base),'GITHUB_REF_NAME':'v0.3.0',
            'GITHUB_REPOSITORY':'example/momus','GITHUB_SHA':'1'*40,'GITHUB_REF':'refs/tags/v0.3.0',
            'TAGS':json.dumps(['v0.2.0','v0.3.0'])}
    def execute(self,**env):
        for name in STEPS:
            run=subprocess.run(['bash','-euo','pipefail','-c',block(name)],cwd=self.base,
                env={**self.env,**env},capture_output=True,text=True)
            if run.returncode: return name,run
        return None,run
    def calls(self):
        p=self.base/'calls'; return [json.loads(x) for x in p.read_text().splitlines()] if p.exists() else []
    def moved(self): return any('/git/refs/' in ' '.join(a) for a in self.calls())
    def test_stable_draft_verify_publish_inventory_then_major_move(self):
        failed,run=self.execute(); self.assertIsNone(failed,run.stdout+run.stderr)
        self.assertTrue(self.moved()); calls=self.calls()
        create=next(a for a in calls if a[:2]==['release','create']); self.assertIn('--draft',create); self.assertIn('--latest=true',create)
        names=json.loads((self.base/'assets').read_text()); self.assertEqual(set(names),policy.assets())
        verify=(self.base/'verify-args').read_text(); self.assertIn('--source-ref refs/tags/v0.3.0',verify)
        final=next(i for i,a in enumerate(calls) if '/releases/tags/' in ' '.join(a) and i>1)
        move=next(i for i,a in enumerate(calls) if '/git/refs/' in ' '.join(a)); self.assertLess(final,move)
    def test_inventory_missing_or_unexpected_blocks_creation(self):
        (self.base/'dist/release-manifest.json').unlink()
        name,run=self.execute(); self.assertEqual(name,'Check the asset inventory'); self.assertFalse((self.base/'draft').exists())
        (self.base/'dist/release-manifest.json').write_text('fixture'); (self.base/'dist/unexpected').touch()
        name,run=self.execute(); self.assertEqual(name,'Check the asset inventory')
    def test_existing_release_or_conflicting_rerun_never_overwrites(self):
        name,run=self.execute(EXISTS='1'); self.assertEqual(name,'Refuse to overwrite an existing release')
        self.assertFalse((self.base/'draft').exists()); self.assertFalse(self.moved())
    def test_lookup_outage_is_not_treated_as_release_absence(self):
        name,run=self.execute(LOOKUP_ERROR='500'); self.assertEqual(name,'Refuse to overwrite an existing release')
        self.assertFalse((self.base/'draft').exists())
    def test_draft_verification_failure_never_publishes_or_moves_major(self):
        name,run=self.execute(VERIFY_FAIL='1'); self.assertEqual(name,'Verify the draft release as a consumer')
        self.assertFalse((self.base/'published').exists()); self.assertFalse(self.moved())
    def test_published_inventory_failure_blocks_major_move(self):
        name,run=self.execute(BAD_PUBLISHED='1'); self.assertEqual(name,'Confirm the published asset set'); self.assertFalse(self.moved())
    def test_published_mutability_blocks_major_move(self):
        name,run=self.execute(PUBLISHED_MUTABLE='1'); self.assertEqual(name,'Confirm the published asset set'); self.assertFalse(self.moved())
    def test_disabled_immutability_blocks_creation(self):
        name,run=self.execute(IMMUTABLE='false'); self.assertEqual(name,'Require immutable releases'); self.assertFalse((self.base/'draft').exists())
    def test_immutability_outage_fails_but_known_permission_boundary_is_explicit(self):
        name,run=self.execute(SETTING_ERROR='500'); self.assertEqual(name,'Require immutable releases')
        name,run=self.execute(SETTING_ERROR='403'); self.assertIsNone(name,run.stderr)
    def test_backport_and_prerelease_do_not_move_major_or_latest(self):
        for tag,tags in [('v0.2.1',['v0.3.0','v0.2.1']),('v0.4.0-rc1',['v0.3.0','v0.4.0-rc1'])]:
            with self.subTest(tag=tag):
                # One execution per fresh fixture so existence checks stay meaningful.
                with self.subTest(fixture="fresh"):
                    self.setUp()
                    name,run=self.execute(GITHUB_REF_NAME=tag,TAGS=json.dumps(tags)); self.assertIsNone(name,run.stderr)
                    self.assertFalse(self.moved()); create=next(a for a in self.calls() if a[:2]==['release','create'])
                    self.assertIn('--latest=false',create); self.assertEqual('--prerelease' in create,'-rc' in tag)
    def test_numeric_version_order_and_other_major_latest(self):
        refs=[{'ref':'refs/tags/v0.9.0'},{'ref':'refs/tags/v0.10.0'},{'ref':'refs/tags/v1.0.0'}]
        self.assertFalse(policy.version('v0.9.0',refs)['advance_major'])
        self.assertTrue(policy.version('v0.10.0',refs)['advance_major'])
        self.assertFalse(policy.version('v0.10.0',refs)['latest'])
    def test_failed_build_sign_audit_verify_and_dispatch_block_publication(self):
        # Interpret the actual job dependency graph and assert publication's
        # event gate, rather than testing a second copy of those dependencies.
        jobs=dict(re.findall(r'^  ([a-z][a-z-]*):\n(.*?)(?=^  [a-z][a-z-]*:|\Z)',WORKFLOW,re.M|re.S))
        graph={}
        for name,body in jobs.items():
            m=re.search(r'^    needs: (.*)$',body,re.M)
            graph[name]=re.findall(r'[a-z][a-z-]*',m[1]) if m else []
        self.assertIn("github.event_name == 'push'",jobs['publish'])
        self.assertIn("startsWith(github.ref, 'refs/tags/')",jobs['publish'])
        for failure in ['build','sign-macos','audit','verify']:
            status={}
            for name in jobs:
                status[name]=name!=failure and all(status[dep] for dep in graph[name])
            self.assertFalse(status['publish'],failure)

if __name__=='__main__': unittest.main()
