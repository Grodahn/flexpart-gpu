#!/usr/bin/env python3
"""Audit issue #123 navigation paths without running scientific validation."""

from __future__ import annotations

import argparse
import re
import shlex
from pathlib import Path
from urllib.parse import unquote, urlsplit

NAVIGATION_FILES = (
    "AGENTS.md",
    ".agents/skills/implementation/SKILL.md",
    ".agents/skills/code-review/SKILL.md",
    ".agents/skills/issue-authoring/SKILL.md",
    ".github/ISSUE_TEMPLATE/implementation.md",
    "docs/agent/repo-map.md",
    "docs/agent/test-map.md",
    "docs/agent/navigation-dry-runs.md",
    "src/gpu/AGENTS.md",
    "src/meteorology/AGENTS.md",
    "src/simulation/AGENTS.md",
    "src/validation/AGENTS.md",
)
LINK = re.compile(r"\[[^\]\n]+\]\(([^)\n]+)\)")
INLINE_CODE = re.compile(r"(?<!`)`([^`\n]+)`(?!`)")


def heading_anchors(text: str) -> set[str]:
    """Recognize GitHub heading anchors, including repeated headings."""
    anchors = set()
    occurrences: dict[str, int] = {}
    for line in text.splitlines():
        match = re.match(r"^#{1,6}\s+(.+?)\s*#*\s*$", line)
        if not match:
            continue
        heading = match.group(1).lower()
        # GitHub drops punctuation while retaining hyphens and underscores.
        base = re.sub(r"[^\w\- ]", "", heading).replace(" ", "-")
        count = occurrences.get(base, 0)
        anchors.add(base if count == 0 else f"{base}-{count}")
        occurrences[base] = count + 1
    return anchors


def local_path(root: Path, source: Path, value: str) -> Path:
    """Resolve only repository-contained navigation targets."""
    target = (source.parent / value).resolve()
    if not target.is_relative_to(root):
        raise ValueError(f"target escapes repository: {value}")
    return target


def check_document(root: Path, relative: str) -> list[str]:
    """Return actionable drift failures for one navigation surface."""
    source = root / relative
    if not source.is_file():
        return [f"{relative}: missing navigation document"]
    content = source.read_text(encoding="utf-8")
    failures = []
    if not content.strip():
        failures.append(f"{relative}: empty navigation document")
    for match in LINK.finditer(content):
        value = match.group(1).strip().strip("<>")
        url = urlsplit(value)
        repository_prefix = "/Grodahn/flexpart-gpu/blob/main/"
        if url.netloc == "github.com" and url.path.startswith(repository_prefix):
            # Issue-template links must also work once copied into an issue body.
            base = root / "command"
            target_path = url.path.removeprefix(repository_prefix)
        elif url.scheme or url.netloc:
            continue
        else:
            base = source
            target_path = url.path
        try:
            target = local_path(root, base, unquote(target_path)) if target_path else source
        except ValueError as error:
            failures.append(f"{relative}: {error}")
            continue
        if not target.exists():
            failures.append(f"{relative}: missing link target {value}")
        elif url.fragment:
            anchor = unquote(url.fragment)
            if not target.is_file() or anchor not in heading_anchors(
                target.read_text(encoding="utf-8")
            ):
                failures.append(f"{relative}: missing heading anchor {value}")
    for match in INLINE_CODE.finditer(content):
        command = match.group(1)
        if not command.startswith(("python ", "python3 ", "bash ", "cargo ")):
            continue
        try:
            args = shlex.split(command)
        except ValueError as error:
            failures.append(f"{relative}: malformed command {command}: {error}")
            continue
        targets: list[str] = []
        if args[0] in ("python", "python3", "bash"):
            if len(args) > 1 and not args[1].startswith("-"):
                targets.append(args[1])
        elif "--test" in args or "--bin" in args:
            for flag, directory in (("--test", "tests"), ("--bin", "src/bin")):
                if flag not in args:
                    continue
                index = args.index(flag) + 1
                if index == len(args):
                    failures.append(f"{relative}: missing {flag} argument")
                else:
                    targets.append(f"{directory}/{args[index]}.rs")
        for value in targets:
            try:
                target = local_path(root, root / "command", value)
                if not target.is_file():
                    failures.append(f"{relative}: missing command target {value}")
            except ValueError as error:
                failures.append(f"{relative}: {error}")
    return failures


def check_navigation(root: Path) -> list[str]:
    """Audit the complete, explicit set so missing surfaces cannot be skipped."""
    root = root.resolve()
    return [
        failure
        for relative in NAVIGATION_FILES
        for failure in check_document(root, relative)
    ]


def main() -> int:
    """Expose a small fail-closed CI command with no third-party dependencies."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    args = parser.parse_args()
    failures = check_navigation(args.root)
    if failures:
        for failure in failures:
            print(f"FAIL: {failure}")
        return 1
    print(f"PASS: {len(NAVIGATION_FILES)} navigation surfaces; links, anchors and command targets exist")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
