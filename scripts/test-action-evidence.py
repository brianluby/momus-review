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
    def test_opt_in_flags_and_literal_arguments(self):
        text = (ROOT / "action.yml").read_text().split("    - id: review\n", 1)[1]
        script = re.search(r"      run: \|\n(.*?)(?=\n    - )", text, re.S).group(1)
        script = textwrap.dedent(script)
        with tempfile.TemporaryDirectory() as raw:
            tmp = Path(raw)
            for name, body in {
                "git": "#!/bin/sh\nprintf '%s\\n' fixture-head\n",
                "momus": "#!/usr/bin/env python3\nimport json,os,sys\nopen(os.environ['CAPTURE'],'w').write(json.dumps(sys.argv[1:]))\n",
            }.items():
                p = tmp / name
                p.write_text(body)
                p.chmod(0o755)
            injection = tmp / "unexpected"
            for enabled in [False, True]:
                env = dict(os.environ, PATH=str(tmp)+os.pathsep+os.environ['PATH'],
                    CAPTURE=str(tmp/'args.json'), RUNNER_TEMP=str(tmp), GITHUB_OUTPUT=str(tmp/'output'),
                    HEAD_SHA='fixture-head', BASE_SHA='base-sha', SCOPES='src tests', SARIF='false',
                    EXCLUDE=f"vendor/**\n$(touch {injection})", UPGRADE_TRIAGE=str(enabled).lower(), DOCS_DRIFT=str(enabled).lower())
                subprocess.run(['bash','-c',script],env=env,check=True,capture_output=True)
                args=json.loads((tmp/'args.json').read_text())
                self.assertEqual('--upgrade-triage' in args,enabled)
                self.assertEqual('--docs-drift' in args,enabled)
                self.assertEqual(args[-3:],['--','src','tests'])
                self.assertIn(f'$(touch {injection})',args)
                self.assertFalse(injection.exists())

if __name__ == '__main__':
    unittest.main()
