"""Offline checks of real IFS fixtures and fail-closed research-audit controls.

Requires ecCodes and numpy; missing prerequisites fail, rather than skip. Full
independent value reproduction is a separate mandatory recorded audit command.
"""

import copy
import hashlib
import json
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parent))
import ifs_open_audit as audit
import ifs_open_comparisons as diagnostics

ROOT = Path(__file__).resolve().parents[2]
FIXTURE = ROOT / "fixtures/meteorology/ifs-open-audit-v1"


class RealSourceTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.manifest = json.loads((FIXTURE / "retrieval-manifest.json").read_text())
        cls.modern = json.loads((FIXTURE / "decoded-2.48.0.json").read_text())
        cls.legacy = json.loads((FIXTURE / "decoded-2.24.2.json").read_text())

    def test_complete_real_inventory_and_vertical_gate(self):
        inventory = audit.validate_inventory(FIXTURE, self.manifest)
        self.assertEqual(len(inventory), 51)
        for item in inventory:
            for param, levels in item["levels_by_parameter"].items():
                self.assertFalse(set(levels) - {"pl", "sfc", "sol"}, param)
                if "pl" in levels:
                    self.assertEqual(levels["pl"], [10, 50, 100, 150, 200, 250, 300, 400, 500, 600, 700, 850, 925, 1000])
        self.assertEqual({r["metadata"]["NV"] for r in self.modern["messages"]}, {0})
        for step in range(0, 49, 3):
            inventories = {resolution: set() for resolution in ("0p1", "0p25")}
            for item in inventory:
                if item["step"] != step:
                    continue
                for param, types in item["levels_by_parameter"].items():
                    for level_type, levels in types.items():
                        for level in levels or [None]:
                            inventories[item["resolution"]].add((param, level_type, level))
            self.assertEqual(inventories["0p1"], inventories["0p25"])

    def test_feasibility_arithmetic_is_bound_to_observed_indexes(self):
        report = json.loads((FIXTURE / "feasibility.json").read_text())
        self.assertEqual(report["regional_one_snapshot_bytes"], 701 * 421 * 93 * 4)
        self.assertEqual(report["regional_two_snapshot_bytes"], 2 * report["regional_one_snapshot_bytes"])
        self.assertEqual(report["regional_17_snapshot_bytes"], 17 * report["regional_one_snapshot_bytes"])
        inventory = audit.validate_inventory(FIXTURE, self.manifest)
        for resolution in ("0p1", "0p25"):
            self.assertEqual(report["archive_index_totals"][resolution],
                             sum(i["file_bytes"] for i in inventory if i["resolution"] == resolution))
        self.assertEqual(report["actual_capture_bytes"], sum(i["bytes"] for i in self.manifest["messages"]))

    def test_all_raw_hashes_metadata_and_independent_evidence(self):
        self.assertEqual(len(self.manifest["messages"]), 34)
        self.assertEqual(len(self.modern["messages"]), 34)
        for item in self.manifest["messages"]:
            record = next(r for r in self.modern["messages"] if r["path"] == item["path"])
            data = (FIXTURE / item["path"]).read_bytes()
            audit.validate_message(data, item, self.manifest, record["metadata"])
            raw = audit.octets.parse(data)
            self.assertEqual(raw, record["octets"])
            audit.compare_metadata(raw, record["metadata"])
            self.assertEqual(record["independent_values"]["max_absolute_difference"], 0.0)
            self.assertEqual(record["independent_values"]["count"], record["values"]["count"])

    def test_two_eccodes_versions_have_identical_numeric_values_but_label_drift(self):
        self.assertEqual(self.modern["ecCodes"], "2.48.0")
        self.assertEqual(self.legacy["ecCodes"], "2.24.2")
        self.assertTrue(self.legacy["legacy_numeric_diagnostic"])
        self.assertEqual(len(self.legacy["messages"]), 34)
        unknown = set()
        for record in self.legacy["messages"]:
            modern = next(r for r in self.modern["messages"] if r["path"] == record["path"])
            self.assertEqual(record["sha256"], modern["sha256"])
            self.assertEqual(record["values"], modern["values"])
            self.assertEqual(record["octets"], modern["octets"])
            self.assertEqual(record["independent_values"]["max_absolute_difference"], 0.0)
            if record["metadata"]["shortName"] == "unknown":
                unknown.add(modern["metadata"]["shortName"])
        self.assertEqual(unknown, {"cp", "ewss", "nsss", "sf", "tcc"})

    def sample(self, name="0p1-3-cp-sfc.grib2"):
        item = next(i for i in self.manifest["messages"] if i["path"] == name)
        record = next(r for r in self.modern["messages"] if r["path"] == name)
        return (FIXTURE / name).read_bytes(), item, record["metadata"]

    def test_wrong_cycle_resolution_level_step_units_packing_and_missing_fail(self):
        data, item, metadata = self.sample()
        for key, wrong in [("dataDate", 20261031), ("dataTime", 1200), ("Ni", 1440), ("level", 850),
                           ("endStep", 6), ("stepUnits", 0), ("units", "kg m-2 s-1"),
                           ("bitsPerValue", 16), ("packingType", "grid_simple"), ("numberOfMissing", 1),
                           ("startStep", 3), ("NV", 276), ("typeOfLevel", "hybrid"),
                           ("typeOfStatisticalProcessing", 0), ("stream", "enfo"), ("type", "ef"),
                           ("jDirectionIncrementInDegrees", 0.25), ("uvRelativeToGrid", 1),
                           ("gridDefinitionTemplateNumber", 40), ("gridType", "reduced_gg"),
                           ("numberOfDataPoints", 10), ("numberOfValues", 10)]:
            with self.subTest(key=key):
                changed = dict(metadata, **{key: wrong})
                with self.assertRaises(ValueError):
                    audit.validate_message(data, item, self.manifest, changed)

    def test_unsupported_manifest_product_and_resolution_fail(self):
        data, item, metadata = self.sample("0p25-0-2t-sfc.grib2")
        for key, wrong in (("model", "aifs-single"), ("stream", "enfo"), ("type", "ef")):
            with self.subTest(key=key):
                manifest = dict(self.manifest, **{key: wrong})
                with self.assertRaisesRegex(ValueError, "unsupported product"):
                    audit.validate_inventory(FIXTURE, manifest)
                with self.assertRaisesRegex(ValueError, "unsupported product"):
                    audit.validate_message(data, item, manifest, metadata)
        with self.assertRaisesRegex(ValueError, "unsupported resolution"):
            audit.validate_message(data, dict(item, resolution="0p9"), self.manifest, metadata)

    def test_comparisons_reject_corrupt_and_cross_cycle_real_messages(self):
        target = FIXTURE / "0p1-0-2t-sfc.grib2"
        original = Path.read_bytes
        data = original(target)
        handle = audit.ec.codes_new_from_message(data)
        try:
            audit.ec.codes_set(handle, "dataDate", 20261031)
            cross_cycle = audit.ec.codes_get_message(handle)
        finally:
            audit.ec.codes_release(handle)
        for replacement in (data[:-1], cross_cycle):
            with self.subTest(size=len(replacement)):
                def read_bytes(path):
                    return replacement if path == target else original(path)
                with patch.object(Path, "read_bytes", read_bytes):
                    with self.assertRaises(ValueError):
                        diagnostics.comparisons(FIXTURE)
        # A re-recorded hash cannot make a message from another cycle input-equivalent.
        manifest = copy.deepcopy(self.manifest)
        item = next(i for i in manifest["messages"] if i["path"] == target.name)
        item.update(sha256=audit.sha(cross_cycle), bytes=len(cross_cycle))
        def read_bytes(path):
            if path == target:
                return cross_cycle
            if path == FIXTURE / "retrieval-manifest.json":
                return json.dumps(manifest).encode()
            return original(path)
        with patch.object(Path, "read_bytes", read_bytes):
            with self.assertRaisesRegex(ValueError, "message identity dataDate"):
                diagnostics.comparisons(FIXTURE)

    def test_actual_octet_cycle_level_grid_precision_and_corruption_mutations(self):
        data, item, _ = self.sample()
        offset, sections = 16, {}
        while offset < len(data) - 4:
            sections[data[offset + 4]] = offset
            offset += int.from_bytes(data[offset:offset + 4], "big")
        for section, index, replacement in [(1, 15, b"\x02"), (3, 30, (1440).to_bytes(4, "big")),
                                             (3, 67, (500000).to_bytes(4, "big")), (3, 54, b"\x38"),
                                             (4, 24, (850).to_bytes(4, "big")), (5, 19, b"\x10")]:
            with self.subTest(section=section, index=index):
                changed = bytearray(data)
                start = sections[section] + index
                changed[start:start + len(replacement)] = replacement
                changed = bytes(changed)
                admitted_hash = dict(item, sha256=hashlib.sha256(changed).hexdigest())
                handle = audit.ec.codes_new_from_message(changed)
                try:
                    metadata = audit.read_metadata(handle)
                    with self.assertRaises(ValueError):
                        audit.validate_message(changed, admitted_hash, self.manifest, metadata)
                finally:
                    audit.ec.codes_release(handle)
        for bad in (data[:-1], data + b"junk", b"no GRIB"):
            with self.assertRaises(ValueError):
                audit.validate_message(bad, item, self.manifest, self.sample()[2])

    def test_cross_run_message_substitution_fails(self):
        data, item, metadata = self.sample()
        changed = copy.deepcopy(self.manifest)
        changed["cycle"] = "2026110112"
        with self.assertRaises(ValueError):
            audit.validate_message(data, item, changed, metadata)
        wrong, _, _ = self.sample("0p1-6-cp-sfc.grib2")
        with self.assertRaises(ValueError):
            audit.validate_message(wrong, item, self.manifest, metadata)

    def test_missing_required_message_or_index_fails_before_decode(self):
        changed = copy.deepcopy(self.manifest)
        changed["indexes"].pop()
        with self.assertRaises(ValueError):
            audit.validate_inventory(FIXTURE, changed)
        # Use real indexes and a deliberately incomplete manifest, no placeholder GRIB.
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            changed = copy.deepcopy(self.manifest)
            changed["messages"].pop()
            (root / "retrieval-manifest.json").write_text(json.dumps(changed))
            for item in changed["indexes"]:
                (root / item["path"]).write_bytes((FIXTURE / item["path"]).read_bytes())
            with self.assertRaisesRegex(ValueError, "representative message"):
                audit.run(root)

    def test_value_diagnostics_regenerate_from_raw(self):
        expected = json.loads((FIXTURE / "value-comparisons.json").read_text())
        self.assertEqual(diagnostics.comparisons(FIXTURE), expected)
        negative = next(r for r in expected["accumulation_diagnostics"] if r["parameter"] == "tp" and r["from"] == 3)
        self.assertEqual(negative["negative_count"], 152510)
        self.assertLess(negative["minimum_delta_m"], 0)


if __name__ == "__main__":
    unittest.main()
