#!/usr/bin/env python3
"""Run the real installer/verifier with fixture releases and a bounded gh double."""
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent.parent
TARGET = 'x86_64-unknown-linux-gnu'
SHA = '1' * 40
GH = r'''#!/usr/bin/env python3
import json, os, pathlib, shutil, sys
args=sys.argv[1:]
with open(os.environ['CALLS'],'a') as f: f.write(json.dumps(args)+'\n')
if args==['--version']: print('gh version 2.101.0'); sys.exit(0)
if args[0]=='api':
    endpoint=args[1]
    if '/releases/' in endpoint:
        print(json.dumps({'tag_name':'v0.3.0','draft':False,'immutable':os.environ.get('MUTABLE')!='1'}))
    elif '/git/ref/' in endpoint:
        print(json.dumps({'object':{'type':'tag' if os.environ.get('ANNOTATED') else 'commit','sha':'1'*40}}))
    elif '/git/tags/' in endpoint:
        print(json.dumps({'object':{'type':'commit','sha':'1'*40}}))
    else: sys.exit(8)
elif args[:2]==['release','download']:
    if os.environ.get('DOWNLOAD_FAIL'): sys.exit(1)
    dest=pathlib.Path(args[args.index('--dir')+1]); fixture=pathlib.Path(os.environ['FIXTURE'])
    for i,v in enumerate(args):
        if v=='--pattern':
            src=fixture/args[i+1]
            if not src.exists(): sys.exit(1)
            shutil.copyfile(src,dest/src.name)
    if os.environ.get('TAMPER'): (dest/'momus-x86_64-unknown-linux-gnu.tar.gz').write_bytes(b'tampered')
elif args[:2]==['attestation','verify']:
    if os.environ.get('REJECT'): sys.exit(1)
    print(json.dumps([{'verificationResult':{'statement':{'predicate':{'bomFormat':'CycloneDX'}}}}]))
else: sys.exit(9)
'''
SOURCE = r'''#!/usr/bin/env python3
import json,os,pathlib,sys
with open(os.environ['SOURCE_CALLS'],'a') as f: f.write(json.dumps({'cwd':os.getcwd(),'args':sys.argv})+'\n')
if pathlib.Path(sys.argv[0]).name=='cargo':
    root=pathlib.Path(sys.argv[sys.argv.index('--root')+1]); p=root/'bin/momus'
    p.write_text('#!/bin/sh\necho source-executed >> "$EXECUTED"\n'); p.chmod(0o755)
'''

class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.tmp=tempfile.TemporaryDirectory(); self.addCleanup(self.tmp.cleanup)
        self.base=Path(self.tmp.name); self.fixture=self.base/'fixture'; self.fixture.mkdir()
        self.bin=self.base/'bin'; self.bin.mkdir()
        for name,body in [('gh',GH),('rustup',SOURCE),('cargo',SOURCE)]:
            p=self.bin/name; p.write_text(body); p.chmod(0o755)
        archive=self.fixture/f'momus-{TARGET}.tar.gz'
        with tarfile.open(archive,'w:gz') as tar:
            data=b'#!/bin/sh\necho binary-executed >> "$EXECUTED"\n'
            info=tarfile.TarInfo(f'momus-{TARGET}/momus'); info.size=len(data); info.mode=0o755
            tar.addfile(info,io.BytesIO(data))
        (self.fixture/(archive.name+'.sha256')).write_text(hashlib.sha256(archive.read_bytes()).hexdigest()+'  '+archive.name+'\n')
        sbom=self.fixture/f'momus-{TARGET}.cdx.json'; sbom.write_text('{"bomFormat":"CycloneDX"}')
        for suffix in ['provenance.bundle.json','sbom-attestation.bundle.json']:
            (self.fixture/f'momus-{TARGET}.{suffix}').write_text('{}')
        manifest=self.fixture/'release-manifest.json'; manifest.write_text(json.dumps({'targets':{TARGET:{
            'archive':{'name':archive.name,'sha256':hashlib.sha256(archive.read_bytes()).hexdigest()},
            'sbom':{'name':sbom.name,'sha256':hashlib.sha256(sbom.read_bytes()).hexdigest()}}}}))
        (self.fixture/'release-manifest.json.sha256').write_text(hashlib.sha256(manifest.read_bytes()).hexdigest())
        self.env={**os.environ,'PATH':str(self.bin)+os.pathsep+os.environ['PATH'],
            'RUNNER_TEMP':str(self.base/'runner'),'RUNNER_OS':'Linux','RUNNER_ARCH':'X64',
            'ACTION_PATH':str(ROOT),'GITHUB_PATH':str(self.base/'path'),'VERSION':'latest',
            'VERIFY_ATTESTATIONS':'required','RELEASE_REPO':'example/momus',
            'FIXTURE':str(self.fixture),'CALLS':str(self.base/'calls'),'EXECUTED':str(self.base/'executed'),
            'SOURCE_CALLS':str(self.base/'source-calls')}
        (self.base/'runner').mkdir()
    def run_install(self,**env):
        return subprocess.run(['bash',str(ROOT/'scripts/install-release.sh')],env={**self.env,**env},capture_output=True,text=True)
    def calls(self):
        p=self.base/'calls'; return [json.loads(x) for x in p.read_text().splitlines()] if p.exists() else []
    def assert_rejected(self,**env):
        run=self.run_install(**env); self.assertNotEqual(run.returncode,0,run.stdout+run.stderr)
        self.assertFalse((self.base/'executed').exists()); self.assertFalse((self.base/'source-calls').exists())
        self.assertFalse((self.base/'runner/momus-dl'/f'momus-{TARGET}').exists())
    def test_required_resolves_latest_once_before_download_and_verifies_custom_repo(self):
        run=self.run_install(); self.assertEqual(run.returncode,0,run.stdout+run.stderr)
        calls=self.calls(); self.assertEqual(sum('/releases/latest' in a[-1] for a in calls),1)
        download=next(a for a in calls if a[:2]==['release','download']); self.assertEqual(download[2],'v0.3.0')
        for call in [a for a in calls if a[:2]==['attestation','verify']]:
            self.assertEqual(call[call.index('--repo')+1],'example/momus')
            self.assertEqual(call[call.index('--signer-workflow')+1],'example/momus/.github/workflows/attest.yml')
            self.assertEqual(call[call.index('--source-digest')+1],SHA)
            self.assertEqual(call[call.index('--source-ref')+1],'refs/tags/v0.3.0')
        self.assertTrue((self.base/'executed').exists()); self.assertFalse((self.base/'source-calls').exists())
    def test_annotated_tag_is_peeled(self):
        run=self.run_install(VERSION='v0.3.0',ANNOTATED='1'); self.assertEqual(run.returncode,0,run.stderr)
        self.assertTrue(any('/git/tags/' in a[-1] for a in self.calls()))
    def test_legacy_needs_only_archive_and_checksum(self):
        for p in self.fixture.iterdir():
            if not (p.name.endswith('.tar.gz') or p.name.endswith('.tar.gz.sha256')): p.unlink()
        run=self.run_install(VERIFY_ATTESTATIONS='legacy',MUTABLE='1'); self.assertEqual(run.returncode,0,run.stderr)
        self.assertFalse(any(a[:2]==['attestation','verify'] for a in self.calls()))
        download=next(a for a in self.calls() if a[:2]==['release','download']); self.assertEqual(download.count('--pattern'),2)
    def test_missing_bundle_fails_without_execution_or_source_fallback(self):
        (self.fixture/f'momus-{TARGET}.provenance.bundle.json').unlink(); self.assert_rejected()
    def test_download_failure_fails_closed(self): self.assert_rejected(DOWNLOAD_FAIL='1')
    def test_checksum_failure_fails_closed(self): self.assert_rejected(TAMPER='1')
    def test_attestation_failure_fails_closed(self): self.assert_rejected(REJECT='1')
    def test_mutable_release_rejected(self): self.assert_rejected(MUTABLE='1')
    def test_invalid_policy_rejected(self): self.assert_rejected(VERIFY_ATTESTATIONS='requird')
    def test_unsupported_platform_requires_explicit_source(self): self.assert_rejected(RUNNER_ARCH='ARM')
    def test_explicit_source_build_uses_action_checkout_and_locked_install(self):
        run=self.run_install(VERSION='source'); self.assertEqual(run.returncode,0,run.stderr)
        calls=[json.loads(x) for x in (self.base/'source-calls').read_text().splitlines()]
        self.assertTrue(all(c['cwd']==str(ROOT) for c in calls)); self.assertIn('--locked',calls[-1]['args'])
        self.assertEqual(self.calls(),[])

if __name__=='__main__': unittest.main()
