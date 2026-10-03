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
LINK = re.compile(
    r"""\[[^\]\n]+\]\((<[^>]+>|[^\s)]+)(?:\s+(?:"[^"]*"|'[^']*'|\([^)]*\)))?\s*\)"""
)
REFERENCE_DEFINITION = re.compile(
    r"^\s{0,3}\[([^\]]+)\]:\s*(<[^>]+>|\S+)(?:\s+.*)?$", re.MULTILINE
)
REFERENCE_LINK = re.compile(r"(?<!!)\[([^\]\n]+)\]\[([^\]\n]*)\]")
SHORTCUT_LINK = re.compile(r"(?<![!\]])\[([^\]\n]+)\](?![(:\[])")
INLINE_CODE = re.compile(r"(?<!`)`([^`\n]+)`(?!`)")
CODE_SPAN = re.compile(r"(`+)([^\n]*?)\1(?!`)")
RAW_STRING_OPEN = re.compile(r'(?:br|r)(#{0,255})"')
CHARACTER_LITERAL = re.compile(
    r"'(?:\\(?:x[0-9A-Fa-f]{2}|u\{[0-9A-Fa-f_]+\}|.)|[^\\'\n])'"
)


def prose_lines(text: str) -> list[str]:
    """Exclude fenced examples so sample headings cannot satisfy real anchors."""
    lines = []
    fence_character = None
    fence_length = 0
    for line in text.splitlines():
        match = re.match(r"^\s{0,3}(`{3,}|~{3,})(.*)$", line)
        if fence_character is None:
            if match:
                fence_character = match.group(1)[0]
                fence_length = len(match.group(1))
                lines.append("")
            else:
                lines.append(line)
        elif (
            match
            and match.group(1)[0] == fence_character
            and len(match.group(1)) >= fence_length
            and not match.group(2).strip()
        ):
            fence_character = None
            lines.append("")
    return lines


def heading_anchors(text: str) -> set[str]:
    """Recognize ATX/Setext heading anchors outside fenced examples."""
    anchors = set()
    occurrences: dict[str, int] = {}
    previous = ""
    for line in prose_lines(text):
        match = re.match(r"^ {0,3}#{1,6}\s+(.+?)\s*#*\s*$", line)
        heading = match.group(1) if match else None
        if heading is None and previous.strip() and re.fullmatch(r" {0,3}(?:=+|-+)\s*", line):
            heading = previous.strip()
        previous = "" if heading is not None else line
        if heading is None:
            continue
        base = re.sub(r"[^\w\- ]", "", heading.lower()).replace(" ", "-")
        count = occurrences.get(base, 0)
        anchor = base if count == 0 else f"{base}-{count}"
        while anchor in anchors:
            count += 1
            anchor = f"{base}-{count}"
        anchors.add(anchor)
        occurrences[base] = count + 1
    return anchors


def reference_label(label: str) -> str:
    """Normalize reference labels according to Markdown whitespace/case rules."""
    return " ".join(label.split()).casefold()


def navigation_links(text: str) -> tuple[list[str], list[str]]:
    """Read inline and reference links without treating fenced examples as prose."""
    prose = "\n".join(prose_lines(text))
    prose = CODE_SPAN.sub(lambda match: " " * len(match.group(0)), prose)
    definitions = {}
    for match in REFERENCE_DEFINITION.finditer(prose):
        # Markdown uses the first definition when a label is repeated.
        definitions.setdefault(reference_label(match.group(1)), match.group(2).strip("<>"))
    links = [match.group(1).strip().strip("<>") for match in LINK.finditer(prose)]
    failures = []
    for match in REFERENCE_LINK.finditer(prose):
        label = reference_label(match.group(2) or match.group(1))
        if label not in definitions:
            failures.append(f"undefined link reference {label}")
        else:
            links.append(definitions[label])
    # A shortcut only becomes a Markdown link when its definition exists.
    for match in SHORTCUT_LINK.finditer(prose):
        label = reference_label(match.group(1))
        if label in definitions:
            links.append(definitions[label])
    return links, failures


def local_path(root: Path, source: Path, value: str) -> Path:
    """Resolve only repository-contained navigation targets."""
    target = (source.parent / value).resolve()
    if not target.is_relative_to(root):
        raise ValueError(f"target escapes repository: {value}")
    return target


def rust_code(text: str) -> str:
    """Mask comments/literals while preserving declaration offsets and braces."""
    masked = list(text)
    index = 0
    while index < len(text):
        start = index
        keep_quotes = False
        if text.startswith("//", index):
            end = text.find("\n", index)
            index = len(text) if end < 0 else end
        elif text.startswith("/*", index):
            depth = 1
            index += 2
            while index < len(text) and depth:
                if text.startswith("/*", index):
                    depth += 1
                    index += 2
                elif text.startswith("*/", index):
                    depth -= 1
                    index += 2
                else:
                    index += 1
        else:
            raw = RAW_STRING_OPEN.match(text, index) if text[index] in "br" else None
            character = CHARACTER_LITERAL.match(text, index) if text[index] == "'" else None
            if raw:
                delimiter = '"' + raw.group(1)
                end = text.find(delimiter, raw.end())
                index = len(text) if end < 0 else end + len(delimiter)
            elif text[index] == '"':
                keep_quotes = True
                index += 1
                while index < len(text):
                    if text[index] == "\\":
                        index += 2
                    elif text[index] == '"':
                        index += 1
                        break
                    else:
                        index += 1
                index = min(index, len(text))
            elif character:
                index = character.end()
            else:
                index += 1
                continue
        for position in range(start, index):
            if text[position] != "\n":
                masked[position] = " "
        if keep_quotes:
            # Path attributes need visible delimiters and unchanged source offsets.
            masked[start] = '"'
            if text[index - 1] == '"':
                masked[index - 1] = '"'
    return "".join(masked)


def scope_match(pattern: str, code: str) -> re.Match[str] | None:
    """Find a declaration only at the current namespace's brace depth."""
    for match in re.finditer(pattern, code):
        preceding = code[:match.start()]
        if preceding.count("{") == preceding.count("}"):
            return match
    return None


def inline_module_end(code: str, opening: int) -> int:
    """Bound an inline module without counting braces in comments/literals."""
    depth = 1
    for position in range(opening + 1, len(code)):
        if code[position] == "{":
            depth += 1
        elif code[position] == "}":
            depth -= 1
            if depth == 0:
                return position
    raise ValueError("unclosed inline module in exact-test source")


def exact_test_failure(root: Path, args: list[str]) -> str | None:
    """Detect obvious exact-selector drift without invoking the Rust test harness."""
    if args[:2] != ["cargo", "test"] or "--exact" not in args:
        return None
    if "--test" in args:
        index = args.index("--test") + 1
        if index >= len(args):
            return None
        source = local_path(root, root / "command", f"tests/{args[index]}.rs")
        selector_index = index + 1
    elif "--lib" in args:
        source = root / "src/lib.rs"
        selector_index = args.index("--lib") + 1
    else:
        return None
    if selector_index >= len(args) or args[selector_index].startswith("-"):
        return "exact test command lacks a named selector"
    selector = args[selector_index]
    parts = selector.split("::")
    if not source.is_file():
        return f"missing exact-test source {source.relative_to(root)}"
    content = source.read_text(encoding="utf-8")
    code = rust_code(content)
    module_directory = source.parent
    attribute_directory = source.parent
    for module in parts[:-1]:
        declaration = scope_match(
            rf'(?:#\[path\s*=\s*"([^"]+)"\]\s*)?(?:pub(?:\([^)]*\))?\s+)?mod\s+{re.escape(module)}\s*([;{{])',
            code,
        )
        if declaration is None:
            return f"missing exact-test module for {selector}"
        if declaration.group(2) == "{":
            opening = declaration.end() - 1
            closing = inline_module_end(code, opening)
            content = content[opening + 1:closing]
            code = code[opening + 1:closing]
            module_directory /= module
            attribute_directory = module_directory
            continue
        if declaration.group(1):
            start, end = declaration.span(1)
            source = local_path(root, attribute_directory / "module", content[start:end])
        else:
            file_source = module_directory / f"{module}.rs"
            source = file_source if file_source.is_file() else module_directory / module / "mod.rs"
            source = local_path(root, root / "command", source.as_posix())
        if not source.is_file():
            return f"missing exact-test source {source.relative_to(root)}"
        content = source.read_text(encoding="utf-8")
        code = rust_code(content)
        module_directory = source.parent if source.name == "mod.rs" else source.with_suffix("")
        attribute_directory = source.parent
    test = scope_match(
        rf"#\[test\]\s*(?:#\[[^\]]+\]\s*)*(?:pub(?:\([^)]*\))?\s+)?fn\s+{re.escape(parts[-1])}\s*\(",
        code,
    )
    if test is None:
        return f"missing exact-test function {selector}"
    return None


def check_document(root: Path, relative: str) -> list[str]:
    """Return actionable drift failures for one navigation surface."""
    source = root / relative
    if not source.is_file():
        return [f"{relative}: missing navigation document"]
    content = source.read_text(encoding="utf-8")
    links, reference_failures = navigation_links(content)
    failures = [f"{relative}: {failure}" for failure in reference_failures]
    if not content.strip():
        failures.append(f"{relative}: empty navigation document")
    for value in links:
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
    prose = "\n".join(prose_lines(content))
    for match in INLINE_CODE.finditer(prose):
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
        try:
            test_failure = exact_test_failure(root, args)
        except ValueError as error:
            test_failure = str(error)
        if test_failure:
            failures.append(f"{relative}: {test_failure}")
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
