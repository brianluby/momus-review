#!/usr/bin/env python3
"""Create deterministic Git review inputs without benchmark labels or receipts."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

from evaluate import validate_manifest


def git_bytes(root, *args):
    # Caller Git overrides can otherwise redirect init/add/commit to an
    # unrelated checkout, index or object store despite the explicit cwd.
    env = {key: value for key, value in os.environ.items() if not key.startswith("GIT_")}
    env.update(GIT_CONFIG_NOSYSTEM="1", GIT_CONFIG_GLOBAL=os.devnull,
               GIT_AUTHOR_DATE="2000-01-01T00:00:00Z",
               GIT_COMMITTER_DATE="2000-01-01T00:00:00Z")
    return subprocess.check_output(
        ["git", "-c", "core.hooksPath=" + os.devnull,
         "-c", "commit.gpgsign=false", "-c", "core.autocrlf=false",
         "-c", "user.name=Momus Fixture", "-c", "user.email=fixture@example.invalid",
         *args], cwd=root, env=env, stderr=subprocess.PIPE)


def git(root, *args):
    return git_bytes(root, *args).decode().strip()


def git_tree_hash(root, commit):
    paths = git_bytes(root, "ls-tree", "-r", "--name-only", "-z", commit).split(b"\0")
    entries = [{"path": path.decode(), "sha256": hashlib.sha256(
        git_bytes(root, "cat-file", "blob", commit + ":" + path.decode())).hexdigest()}
        for path in sorted(paths) if path]
    return hashlib.sha256(json.dumps(entries, sort_keys=True, separators=(",", ":"),
                                     ensure_ascii=False).encode()).hexdigest()


def populate(source, target):
    for path in sorted(source.rglob("*")):
        if path.is_file():
            relative = path.relative_to(source)
            if ".git" in relative.parts or path.is_symlink():
                raise ValueError("Git metadata and symlinks are not fixture inputs")
            output = target / relative
            output.parent.mkdir(parents=True, exist_ok=True)
            output.write_bytes(path.read_bytes())


def materialize(manifest_path, case_id, output, receipt):
    manifest_path = Path(manifest_path).resolve()
    manifest_bytes = manifest_path.read_bytes()
    manifest = validate_manifest(manifest_path)
    if manifest_path.read_bytes() != manifest_bytes or manifest != json.loads(manifest_bytes):
        raise ValueError("Manifest changed during validation")
    case = next((c for c in manifest["cases"] if c["id"] == case_id), None)
    if case is None:
        raise ValueError("Unknown benchmark case")
    output = Path(output).resolve()
    receipt = Path(receipt).resolve()
    if output.exists() or receipt.exists():
        raise ValueError("Output workspace and receipt must be new paths")
    if receipt == output or output in receipt.parents:
        raise ValueError("Receipt must remain outside review inputs")
    if manifest_path.parent == output or manifest_path.parent in output.parents:
        raise ValueError("Review inputs must remain outside the labeled corpus")
    output.parent.mkdir(parents=True, exist_ok=True)
    receipt.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".momus-materialize-", dir=output.parent) as tmp:
        root = Path(tmp)
        populate(manifest_path.parent / case["base"], root)
        git(root, "init", "--initial-branch=fixture")
        git(root, "add", "--force", "--all")
        git(root, "commit", "--allow-empty", "-m", "Base input")
        base = git(root, "rev-parse", "HEAD")
        for path in root.iterdir():
            if path.name != ".git":
                shutil.rmtree(path) if path.is_dir() else path.unlink()
        populate(manifest_path.parent / case["head"], root)
        git(root, "add", "--force", "--all")
        git(root, "commit", "--allow-empty", "-m", "Review input")
        head = git(root, "rev-parse", "HEAD")
        if git_tree_hash(root, base) != case["source"]["baseSha256"] or \
                git_tree_hash(root, head) != case["source"]["headSha256"]:
            raise ValueError("Copied Git source objects differ from the validated source snapshots")
        result = {
            "schemaVersion": 2, "datasetVersion": manifest["datasetVersion"],
            "manifestSha256": hashlib.sha256(manifest_bytes).hexdigest(),
            "caseId": case_id,
            "source": {key: case["source"][key] for key in
                       ("baseSha256", "headSha256", "diffSha256")},
            "reviewedBase": base, "reviewedHead": head,
            "reviewedScope": str(output),
        }
        # Roll back only a receipt exclusively created by this call, including
        # failures during its write or close.
        created_receipt = False
        try:
            with receipt.open("x") as stream:
                created_receipt = True
                stream.write(json.dumps(result, indent=2, sort_keys=True) + "\n")
            os.rename(root, output)
        except BaseException:
            if created_receipt:
                receipt.unlink()
            raise
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=Path(__file__).with_name("manifest.json"))
    parser.add_argument("--case", required=True)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--receipt", required=True, type=Path)
    args = parser.parse_args()
    try:
        print(json.dumps(materialize(args.manifest, args.case, args.output, args.receipt), indent=2))
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(2, f"materialization failed: {error}\n")


if __name__ == "__main__":
    main()
