#!/usr/bin/env python3
"""Exercise the actual composite review shell with opt-in inputs and a stub CLI."""
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
import textwrap
import unittest

ROOT = Path(__file__).resolve().parents[1]

class ActionEvidenceTests(unittest.TestCase):
    def review_step(self):
        text = (ROOT / "action.yml").read_text()
        match = re.search(r"^    - id: review\n(.*?)(?=^    - |\Z)", text, re.M | re.S)
        self.assertIsNotNone(match, "action.yml must contain the review step")
        step = match.group(1)
        run = re.search(r"^      run: \|\n(.*)", step, re.M | re.S)
        self.assertIsNotNone(run, "review step must contain a literal shell block")
        env = dict(re.findall(r"^        ([A-Z_]+): (.*)$", step, re.M))
        for name, expected in {
            "UPGRADE_TRIAGE": "${{ inputs.upgrade-triage }}",
            "DOCS_DRIFT": "${{ inputs.docs-drift }}",
            "BASE_SHA": "${{ github.event.pull_request.base.sha }}",
            "HEAD_SHA": "${{ github.event.pull_request.head.sha }}",
            "SCOPES": "${{ inputs.paths }}",
            "EXCLUDE": "${{ inputs.exclude }}",
            "SARIF": "${{ inputs.sarif }}",
        }.items():
            self.assertEqual(env.get(name), expected, f"review env mapping for {name}")
        return textwrap.dedent(run.group(1))

    def test_opt_in_flags_and_literal_arguments(self):
        script = self.review_step()
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            binaries = tmp / "bin"
            binaries.mkdir()
            for name, body in {
                "git": "#!/bin/sh\n[ \"$1 $2\" = 'rev-parse HEAD' ] || exit 2\nprintf '%s\\n' fixture-head\n",
                "momus": "#!/usr/bin/env python3\nimport json,os,sys\nopen(os.environ['CAPTURE'],'w').write(json.dumps(sys.argv[1:]))\n",
            }.items():
                p = binaries / name
                p.write_text(body)
                p.chmod(0o755)
            injection = tmp / "unexpected"
            for upgrade, drift in [(False, False), (True, False), (False, True), (True, True)]:
                for head in ['fixture-head', 'mismatched-head']:
                    with self.subTest(upgrade=upgrade, drift=drift, head=head):
                        env = dict(os.environ, PATH=str(binaries)+os.pathsep+os.environ['PATH'],
                            CAPTURE=str(tmp/'args.json'), RUNNER_TEMP=str(tmp), GITHUB_OUTPUT=str(tmp/'output'),
                            HEAD_SHA=head, BASE_SHA='base-sha', SCOPES='src tests', SARIF='true',
                            EXCLUDE=f"vendor/**\n$(touch {injection})", UPGRADE_TRIAGE=str(upgrade).lower(), DOCS_DRIFT=str(drift).lower())
                        run = subprocess.run(['bash','--noprofile','--norc','-eo','pipefail','-c',script],
                            env=env,capture_output=True,text=True)
                        self.assertEqual(run.returncode, 0, run.stdout + run.stderr)
                        args=json.loads((tmp/'args.json').read_text())
                        self.assertEqual('--upgrade-triage' in args,upgrade)
                        self.assertEqual('--docs-drift' in args,drift)
                        self.assertEqual(args[:3], ['review', '--base', 'base-sha'])
                        self.assertEqual(args[-3:],['--','src','tests'])
                        self.assertIn(f'$(touch {injection})',args)
                        self.assertFalse(injection.exists())
                        self.assertTrue((tmp/'momus').is_dir())
                        self.assertIn(str(tmp/'momus'/'momus.sarif'), args)
                        self.assertEqual('::warning::HEAD is not the PR head' in run.stdout,
                            head != 'fixture-head')

if __name__ == '__main__':
    unittest.main()
