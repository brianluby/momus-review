#!/usr/bin/env python3
"""Run authored, offline behavioral controls; never invoke manifest commands.

Success means the expected bug/mutation is observed and its paired control is
sound. It is annotation support, not a model evaluation or detection metric.
"""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent
RUST_PAIRS = {"rust-invoice", "rust-authority", "rust-backoff", "rust-wire"}
JS_PAIRS = {"js-tenant", "js-drain", "js-pagination", "js-refund-tests", "js-async-lease", "dependency-codec-api"}
PYTHON_PAIRS = {"python-cents", "python-export", "python-config", "python-signature-tests"}
PAIRS = RUST_PAIRS | JS_PAIRS | PYTHON_PAIRS | {"docs-client-arity"}


class ReproductionError(Exception):
    """A required control or expected failure did not occur."""


def run(argv: list[str], cwd: Path) -> subprocess.CompletedProcess:
    try:
        return subprocess.run(argv, cwd=cwd, capture_output=True, text=True,
                              timeout=20, check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ReproductionError(f"could not run {argv[0]}: {error}") from error


def require_success(process: subprocess.CompletedProcess, name: str) -> None:
    if process.returncode != 0:
        raise ReproductionError(f"{name} failed: {process.stderr.strip()[:2000]}")


def require_expected_failure(process: subprocess.CompletedProcess, markers: tuple[str, ...]) -> None:
    if process.returncode == 0 or not all(marker in process.stderr for marker in markers):
        raise ReproductionError(f"expected failure {markers!r}, got status {process.returncode}: {process.stderr.strip()[:2000]}")


def tool(name: str) -> str:
    executable = shutil.which(name)
    if not executable:
        raise ReproductionError(f"required offline tool unavailable: {name}")
    return executable


def check_tree(directory: Path) -> None:
    if directory.is_symlink() or not directory.is_dir():
        raise ReproductionError(f"fixture is not a real directory: {directory}")
    if any(path.is_symlink() for path in directory.rglob("*")):
        raise ReproductionError(f"fixture contains a symlink: {directory}")


def probe_javascript(pair: str) -> str:
    prelude = 'const assert = require("node:assert/strict");\n'
    probes = {
        "js-tenant": 'const notes = require("./notes");\nconst rows = [{tenant:"blue",id:9,title:"private"}];\nassert.equal(notes.updateTitle(rows,"red",9,"changed"),false,"cross-tenant update");\nassert.equal(rows[0].title,"private");\n',
        "js-drain": 'const queue = ["a","b","c"];\nassert.deepEqual(require("./dispatcher").dispatch(queue),["a","b","c"],"all queued jobs");\nassert.equal(queue.length,0);\n',
        "js-pagination": 'assert.equal(require("./collector").firstCursor([1,2,3,4]),2,"legacy nextCursor");\nassert.equal(require("./catalog").page([1,2,3,4],0).hasMore,true);\n',
        "js-async-lease": '(async () => {\n  const {Lease} = require("./lease");\n  const {forward} = require("./forward");\n  assert.equal(await require("./worker").handle("ping"), "sent:ping");\n  const success = new Lease();\n  assert.equal(await forward(success, "pong"), "sent:pong");\n  assert.equal(success.released, true);\n  const failed = new Lease();\n  await assert.rejects(forward(failed, "fail"), /transport rejected send/);\n  assert.equal(failed.released, true);\n})().catch(error => { console.error(error); process.exitCode = 1; });\n',
        "dependency-codec-api": 'assert.equal(require("./app").writeRecord({id:7}),\'{"id":7}\');\n',
    }
    return prelude + probes[pair]


def probe_python(pair: str) -> str:
    return {
        "python-cents": 'from importer import import_price\nassert import_price({"price":"0.29"})["cents"] == 29, "exact cents: expected 29"\nassert import_price({"price":"9999999.99"})["cents"] == 999999999\nassert import_price({"price":"0.00"})["cents"] == 0\n',
        "python-export": 'from pathlib import Path\nfrom handler import download_path\nroot = Path("/exports")\ntry:\n    download_path(root,{"name":"../private.csv"})\nexcept ValueError:\n    pass\nelse:\n    raise AssertionError("traversal must be rejected")\nassert download_path(root,{"name":"month/report.csv"}) == Path("/exports/month/report.csv")\n',
        "python-config": 'from scheduler import job_timeout\nassert job_timeout({"timeout":2}) == 2, "legacy timeout: expected 2 seconds"\nassert job_timeout({"timeout_ms":2500}) == 2.5\n',
    }[pair]


def mutation_control(pair: str, head: Path, defect: bool, work: Path) -> str:
    if pair == "js-refund-tests":
        argv, file = [tool("node"), "tests.js"], "ledger.js"
        before, after = "  if (state.refunds.has(key)) return false;\n", ""
        rejection_markers = ("ERR_ASSERTION",)
    else:
        argv, file = [sys.executable, "-B", "tests.py"], "webhook.py"
        before, after = "    return hmac.compare_digest(expected, supplied)\n", "    return True\n"
        rejection_markers = ("AssertionError",)
    require_success(run(argv, head), "unmutated fixture tests")
    mutant = work / "mutant"
    shutil.copytree(head, mutant)
    source = (mutant / file).read_text(encoding="utf-8")
    if source.count(before) != 1:
        raise ReproductionError("expected mutation site must occur exactly once")
    (mutant / file).write_text(source.replace(before, after), encoding="utf-8")
    result = run(argv, mutant)
    if defect:
        require_success(result, "surviving contract-violating mutant")
        return "original tests pass; contract-violating mutant survives the missing regression test"
    require_expected_failure(result, rejection_markers)
    return "original tests pass; the paired regression tests reject the contract-violating mutant"


def reproduce(case_id: str) -> dict:
    if case_id.endswith("-defect"):
        pair, variant, defect = case_id.removesuffix("-defect"), "defect", True
    elif case_id.endswith("-fixed"):
        pair, variant, defect = case_id.removesuffix("-fixed"), "fixed", False
    else:
        raise ReproductionError(f"case has no behavioral control: {case_id}")
    if pair not in PAIRS:
        raise ReproductionError(f"unknown authored fixture: {case_id}")
    head = ROOT / "fixtures" / pair / variant
    check_tree(head)
    with tempfile.TemporaryDirectory(prefix="momus-behavior-") as temporary:
        work = Path(temporary)
        if pair in {"js-refund-tests", "python-signature-tests"}:
            observation = mutation_control(pair, head, defect, work)
        elif pair in RUST_PAIRS:
            binary = work / "probe"
            compilation = run([tool("rustc"), "--edition=2021", "-C", "overflow-checks=yes",
                               str(head / "src/main.rs"), "-o", str(binary)], head)
            require_success(compilation, "Rust probe compilation")
            result = run([str(binary)], head)
            if defect:
                markers = ("attempt to shift left with overflow",) if pair == "rust-backoff" else ("assertion", "failed")
                require_expected_failure(result, markers)
            else:
                require_success(result, "fixed Rust contract probe")
            observation = "contract trigger fails as annotated" if defect else "paired implementation satisfies the contract trigger"
        elif pair == "docs-client-arity":
            readme = (head / "README.md").read_text(encoding="utf-8")
            blocks = readme.split("```rust\n")
            if len(blocks) != 2 or "```" not in blocks[1]:
                raise ReproductionError("expected exactly one Rust documentation block")
            # README explicitly describes an excerpt whose caller imports the API.
            prelude = "mod client;\nuse client::connect;\n"
            (work / "main.rs").write_text(prelude + blocks[1].split("```", 1)[0], encoding="utf-8")
            shutil.copyfile(head / "src/client.rs", work / "client.rs")
            result = run([tool("rustc"), "--edition=2021", str(work / "main.rs"),
                          "-o", str(work / "docs-probe")], work)
            if defect:
                require_expected_failure(result, ("E0061", "2 arguments but 1 argument was supplied"))
            else:
                require_success(result, "updated quickstart compilation")
                require_success(run([str(work / "docs-probe")], work), "updated quickstart execution")
            observation = "quickstart fails with the annotated arity mismatch" if defect else "updated quickstart compiles and executes"
        elif pair in JS_PAIRS:
            result = run([tool("node"), "-e", probe_javascript(pair)], head)
            if defect:
                markers = (("TypeError", "codec.encode is not a function") if pair == "dependency-codec-api"
                           else ("lease released before send completed",) if pair == "js-async-lease"
                           else ("ERR_ASSERTION",))
                require_expected_failure(result, markers)
            else:
                require_success(result, "fixed JavaScript contract probe")
            observation = "contract trigger fails as annotated" if defect else "paired implementation satisfies the contract trigger"
        elif pair in PYTHON_PAIRS:
            result = run([sys.executable, "-B", "-c", probe_python(pair)], head)
            if defect:
                markers = {"python-cents": ("AssertionError: exact cents",),
                           "python-export": ("AssertionError: traversal must be rejected",),
                           "python-config": ("AssertionError: legacy timeout",)}[pair]
                require_expected_failure(result, markers)
            else:
                require_success(result, "fixed Python contract probe")
            observation = "contract trigger fails as annotated" if defect else "paired implementation satisfies the contract trigger"
        else:
            raise ReproductionError(f"no code-owned probe for {case_id}")
    return {"caseId": case_id, "status": "validated", "observation": observation}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--case", action="append", help="known authored case id; repeatable")
    parser.add_argument("--all", action="store_true", help="run every paired authored control")
    args = parser.parse_args()
    if args.all == bool(args.case):
        parser.error("choose --all or one or more --case values")
    cases = [f"{pair}-{variant}" for pair in sorted(PAIRS) for variant in ("defect", "fixed")] if args.all else args.case
    results = []
    for case in cases:
        try:
            result = reproduce(case)
        except ReproductionError as error:
            result = {"caseId": case, "status": "error", "error": str(error)}
        results.append(result)
        print(json.dumps(result, sort_keys=True), flush=True)
    return int(any(result["status"] != "validated" for result in results))


if __name__ == "__main__":
    raise SystemExit(main())
