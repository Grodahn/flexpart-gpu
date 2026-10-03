#!/usr/bin/env python3
"""Prove that navigation drift fails even when the rest of a document is valid."""

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

from check_agent_navigation import NAVIGATION_FILES, check_document, check_navigation, heading_anchors


class NavigationTests(unittest.TestCase):
    def setUp(self):
        self.workspace = tempfile.TemporaryDirectory()
        self.addCleanup(self.workspace.cleanup)
        self.root = Path(self.workspace.name).resolve()
        (self.root / "docs").mkdir()
        (self.root / "docs/map.md").write_text("# Map\n", encoding="utf-8")

    def write_map(self, text):
        (self.root / "docs/map.md").write_text(text, encoding="utf-8")

    def test_valid_relative_path_anchor_and_external_link(self):
        self.write_map("[local](../AGENTS.md#hard-rules) [web](https://example.org)\n")
        (self.root / "AGENTS.md").write_text("# Hard rules\n", encoding="utf-8")
        self.assertEqual(check_document(self.root, "docs/map.md"), [])

    def test_missing_link_and_anchor_fail(self):
        self.write_map("[gone](gone.rs) [stale](map.md#missing)\n")
        failures = check_document(self.root, "docs/map.md")
        self.assertEqual(len(failures), 2)
        self.assertIn("missing link target", failures[0])
        self.assertIn("missing heading anchor", failures[1])

    def test_removed_navigation_document_cannot_pass(self):
        failures = check_navigation(self.root)
        self.assertTrue(any("missing navigation document" in value for value in failures))

    def test_empty_document_fails(self):
        self.write_map("")
        self.assertIn("empty navigation document", check_document(self.root, "docs/map.md")[0])

    def test_repository_escape_fails(self):
        self.write_map("[outside](../../outside.md)\n`python ../outside.py`\n")
        failures = check_document(self.root, "docs/map.md")
        self.assertEqual(len(failures), 2)
        self.assertTrue(all("escapes repository" in value for value in failures))

    def test_removed_command_targets_fail(self):
        self.write_map(
            "`cargo test --test removed`\n"
            "`cargo run --bin removed`\n"
            "`python scripts/removed.py`\n"
            "`bash scripts/removed.sh`\n"
        )
        failures = check_document(self.root, "docs/map.md")
        self.assertEqual(len(failures), 4)
        self.assertTrue(all("missing command target" in value for value in failures))

    def test_valid_command_targets_and_ordinary_inline_code(self):
        (self.root / "tests").mkdir()
        (self.root / "tests/real.rs").write_text("", encoding="utf-8")
        (self.root / "scripts").mkdir()
        (self.root / "scripts/real.py").write_text("", encoding="utf-8")
        self.write_map("`cargo test --test real` `python scripts/real.py` `Snapshot`")
        self.assertEqual(check_document(self.root, "docs/map.md"), [])

    def test_duplicate_and_punctuation_anchors(self):
        self.assertEqual(
            heading_anchors("# Issue Definition & Task Slicing\n## Rules\n## Rules\n"),
            {"issue-definition--task-slicing", "rules", "rules-1"},
        )


    def test_issue_template_repository_link_is_checked_locally(self):
        self.write_map("[map](https://github.com/Grodahn/flexpart-gpu/blob/main/docs/missing.md)")
        self.assertIn("missing link target", check_document(self.root, "docs/map.md")[0])

    def test_cli_rejects_missing_link_with_nonzero_exit(self):
        for relative in NAVIGATION_FILES:
            path = self.root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text("# Navigation\n", encoding="utf-8")
        (self.root / "AGENTS.md").write_text("[gone](missing.md)\n", encoding="utf-8")
        result = subprocess.run(
            [sys.executable, str(Path(__file__).with_name("check_agent_navigation.py")),
             "--root", str(self.root)],
            capture_output=True, text=True, check=False,
        )
        self.assertEqual(result.returncode, 1)
        self.assertIn("missing link target missing.md", result.stdout)


if __name__ == "__main__":
    unittest.main()
