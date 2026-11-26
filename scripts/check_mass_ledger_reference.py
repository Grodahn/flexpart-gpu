"""Check the #38 source crosswalk against the pinned git object, without building FLEXPART."""

import argparse
import json
from pathlib import Path
import subprocess


def check(reference, repository):
    def git(*args):
        return subprocess.check_output(["git", "--no-replace-objects", "-C", str(repository), *args])

    revision = reference["revision"]
    if revision != "c70586c2b7f5258850705325881c61f557ea9bd8":
        raise ValueError("unexpected FLEXPART revision")
    if git("rev-parse", "--verify", revision + "^{commit}").decode().strip() != revision:
        raise ValueError("missing pinned FLEXPART commit")
    anchors = reference["reference_anchors"]
    if not anchors:
        raise ValueError("empty source crosswalk")
    sources = {}
    for name, object_id, line, snippet in anchors:
        if name not in sources:
            path = f"{revision}:src/{name}"
            resolved = git("rev-parse", "--verify", path).decode().strip()
            lines = git("cat-file", "blob", resolved).decode("utf-8", errors="replace").splitlines()
            sources[name] = resolved, lines
        resolved, lines = sources[name]
        if resolved != object_id or not 1 <= line <= len(lines) or snippet not in lines[line - 1]:
            raise ValueError(f"source evidence mismatch at {name}:{line}")
    return len(anchors)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repository", required=True, type=Path)
    args = parser.parse_args()
    manifest = Path(__file__).resolve().parents[1] / "fixtures/mass-ledger/reference-v1.json"
    try:
        reference = json.loads(manifest.read_text(encoding="utf-8"))
        count = check(reference, args.repository)
    except (OSError, ValueError, subprocess.CalledProcessError) as error:
        parser.exit(1, f"{error}\n")
    print(json.dumps({
        "status": "source_crosswalk_verified",
        "revision": reference["revision"],
        "anchors": count,
    }))


if __name__ == "__main__":
    main()
