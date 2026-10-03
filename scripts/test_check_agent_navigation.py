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

    def test_reference_links_audit_missing_targets_and_anchors(self):
        self.write_map(
            "[full][missing] [collapsed][] [Shortcut]\n"
            "[missing]: absent.md\n"
            "[collapsed]: map.md#absent\n"
            "[shortcut]: absent-too.md\n"
        )
        failures = check_document(self.root, "docs/map.md")
        self.assertEqual(len(failures), 3)
        self.assertTrue(any("missing heading anchor" in value for value in failures))

    def test_undefined_explicit_reference_fails(self):
        self.write_map("[domain][undefined]\n")
        self.assertIn("undefined link reference", check_document(self.root, "docs/map.md")[0])

    def test_valid_reference_labels_ignore_case_and_repeated_whitespace(self):
        self.write_map("[domain][  Hard   RULES ]\n[hard rules]: ../AGENTS.md#hard-rules\n")
        (self.root / "AGENTS.md").write_text("# Hard rules\n", encoding="utf-8")
        self.assertEqual(check_document(self.root, "docs/map.md"), [])

    def test_fenced_example_heading_cannot_satisfy_an_anchor(self):
        self.write_map("[example](map.md#example)\n```markdown\n# Example\n```\n")
        self.assertIn("missing heading anchor", check_document(self.root, "docs/map.md")[0])

    def test_setext_headings_and_fenced_links(self):
        self.write_map(
            "Real heading\n============\n[real](map.md#real-heading)\n"
            "~~~markdown\n[example](absent.md)\n~~~\n"
        )
        self.assertEqual(check_document(self.root, "docs/map.md"), [])

    def test_exact_selector_rejects_a_renamed_test_in_an_existing_target(self):
        (self.root / "tests").mkdir()
        (self.root / "tests/real.rs").write_text("#[test]\nfn present() {}\n", encoding="utf-8")
        self.write_map("`cargo test --test real removed -- --exact`")
        self.assertIn("missing exact-test function", check_document(self.root, "docs/map.md")[0])

    def test_exact_selector_follows_explicit_integration_module_paths(self):
        (self.root / "tests/cases").mkdir(parents=True)
        (self.root / "tests/integration.rs").write_text(
            '#[path = "cases/real.rs"]\nmod domain;\n', encoding="utf-8",
        )
        (self.root / "tests/cases/real.rs").write_text("#[test]\nfn present() {}\n", encoding="utf-8")
        self.write_map("`cargo test --test integration domain::present -- --exact`")
        self.assertEqual(check_document(self.root, "docs/map.md"), [])

    def test_exact_library_selector_follows_conventional_and_inline_modules(self):
        (self.root / "src/domain").mkdir(parents=True)
        (self.root / "src/lib.rs").write_text("pub mod domain;\n", encoding="utf-8")
        (self.root / "src/domain/mod.rs").write_text(
            "mod tests {\n#[test]\nfn present() {}\n}\n", encoding="utf-8",
        )
        self.write_map("`cargo test --lib domain::tests::present -- --exact`")
        self.assertEqual(check_document(self.root, "docs/map.md"), [])

    def test_exact_selector_rejects_missing_namespace(self):
        (self.root / "tests").mkdir()
        (self.root / "tests/real.rs").write_text("#[test]\nfn present() {}\n", encoding="utf-8")
        self.write_map("`cargo test --test real removed::present -- --exact`")
        self.assertIn("missing exact-test module", check_document(self.root, "docs/map.md")[0])

    def test_repeated_reference_uses_first_definition(self):
        self.write_map("[domain][owner]\n[owner]: absent.md\n[OWNER]: map.md\n")
        self.assertIn("missing link target absent.md", check_document(self.root, "docs/map.md")[0])


if __name__ == "__main__":
    unittest.main()
