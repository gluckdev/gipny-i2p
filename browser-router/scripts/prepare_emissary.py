"""Prepare the pinned emissary source plus reviewed browser patch; never build.

Refuse unexpected local edits instead of replacing or resetting a checkout.
"""
import hashlib
import json
from pathlib import Path
import re
import subprocess


def main():
    root = Path(__file__).resolve().parents[1]
    source = json.loads((root / "emissary-source.json").read_text())
    revision = source["revision"]
    if source["repository"] != "https://github.com/eepnet/emissary" or not re.fullmatch(r"[0-9a-f]{40}", revision):
        raise RuntimeError("unexpected emissary source or incomplete revision")
    patch = root / source["patch"]
    checkout = root / "upstream/emissary"

    def git(*arguments, capture=False):
        command = ["git", "-C", str(checkout), *arguments]
        return subprocess.check_output(command) if capture else subprocess.check_call(command)

    if not checkout.exists():
        checkout.mkdir(parents=True)
        git("init", "--quiet")
        git("remote", "add", "origin", source["repository"])
    if git("remote", "get-url", "origin", capture=True).decode().strip() != source["repository"]:
        raise RuntimeError("unexpected source remote")
    try:
        git("rev-parse", "--verify", "--quiet", "HEAD", capture=True)
    except subprocess.CalledProcessError:
        if any(path.name != ".git" for path in checkout.iterdir()):
            raise RuntimeError("uninitialized source checkout contains files; inspect it manually")
        git("fetch", "--depth=1", "origin", revision)
        git("checkout", "--quiet", "--detach", revision)
    if git("rev-parse", "HEAD", capture=True).decode().strip() != revision:
        raise RuntimeError("unexpected checkout HEAD; preserve and inspect it manually")
    if git("ls-files", "--others", "--exclude-standard", capture=True).strip():
        raise RuntimeError("unexpected untracked source files")
    actual = git("diff", "--binary", "--no-ext-diff", "--no-color", "HEAD", capture=True)
    expected = patch.read_bytes()
    previous = root / "emissary-provenance.json"
    if actual and actual != expected and previous.exists():
        receipt = json.loads(previous.read_text())
        if receipt.get("revision") == revision and receipt.get("actual_diff_sha256") == hashlib.sha256(actual).hexdigest():
            # Update only a previously recorded patch, never unknown edits.
            command = ["git", "-C", str(checkout), "apply", "--reverse"]
            subprocess.run(command + ["--check"], input=actual, check=True)
            subprocess.run(command, input=actual, check=True)
            actual = b""
    if not actual:
        git("apply", "--check", str(patch))
        git("apply", str(patch))
        actual = git("diff", "--binary", "--no-ext-diff", "--no-color", "HEAD", capture=True)
    if actual != expected:
        raise RuntimeError("source edits differ from the reviewed patch")
    digest = hashlib.sha256(expected).hexdigest()
    receipt = dict(source, patch_sha256=digest, actual_diff_sha256=hashlib.sha256(actual).hexdigest())
    (root / "emissary-provenance.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(f"emissary {revision}, reviewed patch SHA256 {digest}")


if __name__ == "__main__":
    main()
