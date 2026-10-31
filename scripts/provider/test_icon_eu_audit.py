#!/usr/bin/env python3
"""Self-tests for the #230 ICON-EU audit tooling, contract and fixtures.

The always-run tests use only the committed raw GRIB2 bytes and the
independent octet reader, so they need neither ecCodes nor network access.
Negative integrity tests mutate copies of real messages; the mutated bytes
exist only in temporary directories. Decoder cross-checks against ecCodes and
libaec run when those libraries are installed and are skipped otherwise.

Run: python -I scripts/provider/test_icon_eu_audit.py
"""

import hashlib
import json
import os
import re
import shutil
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, HERE)
import icon_eu_audit as audit  # noqa: E402
import icon_eu_grib2_sections as sections  # noqa: E402
import icon_eu_fetch as fetch  # noqa: E402

FIXTURE = os.path.join(ROOT, "fixtures", "meteorology", "icon-eu-audit-v1")
RAW = os.path.join(FIXTURE, "raw")
CYCLE = "2026103100"
PREFIX = "icon-eu_europe_regular-lat-lon_"
SNOW_CON = [f"{PREFIX}single-level_{CYCLE}_{step:03d}_SNOW_CON.grib2" for step in range(4)]
CLASSIFICATIONS = {"SUPPORTED_AS_IS", "SUPPORTED_WITH_PROVEN_TRANSFORMATION",
                   "REQUIRES_SCIENTIFIC_DECISION", "MISSING_SOURCE", "UNSUPPORTED"}
SUPPORTED = {"SUPPORTED_AS_IS", "SUPPORTED_WITH_PROVEN_TRANSFORMATION"}
# Offsets inside a section (0-based index of WMO octet n is n-1).
SECTION3_NI = 30
SECTION4_PARAMETER_NUMBER = 10
SECTION4_TIME_UNIT = 17
SECTION4_FORECAST_TIME = 18
SECTION4_STATISTICAL_PROCESSING = 46
SECTION6_BITMAP = 5


def load_json(*parts):
    with open(os.path.join(FIXTURE, *parts), encoding="utf-8") as handle:
        return json.load(handle)


def raw_bytes(cycle, name):
    with open(os.path.join(RAW, cycle, name), "rb") as handle:
        return handle.read()


def section_start(message, number):
    """Absolute byte offset of a section, found by walking section lengths."""
    offset = 16
    while offset < len(message) - 4:
        if message[offset + 4] == number:
            return offset
        offset += int.from_bytes(message[offset:offset + 4], "big")
    raise AssertionError(f"section {number} not found")


def mutate(message, number, index, new_bytes):
    data = bytearray(message)
    start = section_start(message, number) + index
    data[start:start + len(new_bytes)] = new_bytes
    return bytes(data)


class Sample:
    """A temporary sample directory with chosen (name, bytes) files."""

    def __init__(self, files):
        self.directory = tempfile.mkdtemp(prefix="icon-eu-230-")
        for name, data in files:
            with open(os.path.join(self.directory, name), "wb") as handle:
                handle.write(data)

    def __enter__(self):
        return self.directory

    def __exit__(self, *_):
        shutil.rmtree(self.directory)


def snow_files(replace=None, drop=None):
    files = []
    for step, name in enumerate(SNOW_CON):
        if step == drop:
            continue
        data = raw_bytes(CYCLE, name)
        if replace and step in replace:
            data = replace[step](data)
        files.append((name, data))
    return files


class ContractCheckTest(unittest.TestCase):
    """Fail-closed integrity checks on real messages and mutated copies."""

    @classmethod
    def setUpClass(cls):
        cls.contract = load_json("provider-contract.json")

    def violations(self, files, profile="raw_fixture"):
        with Sample(files) as directory:
            return audit.check_sample(directory, self.contract, profile)

    def assert_rejected(self, files, pattern, profile="raw_fixture"):
        found = self.violations(files, profile)
        self.assertTrue(any(re.search(pattern, item) for item in found), found)

    def test_icon_eu_raw_fixture_passes_contract(self):
        self.assertEqual(self.violations(snow_files()), [])

    def test_icon_eu_constant_hhl_messages_pass_contract(self):
        files = [(f"{PREFIX}time-invariant_{CYCLE}_{level}_HHL.grib2",
                  raw_bytes(CYCLE, f"{PREFIX}time-invariant_{CYCLE}_{level}_HHL.grib2")) for level in (1, 10)]
        self.assertEqual(self.violations(files + snow_files()), [])

    def test_icon_eu_corrupted_end_section_rejected(self):
        self.assert_rejected(snow_files({1: lambda data: data[:-4] + b"7776"}), "corrupt message")

    def test_icon_eu_truncated_message_rejected(self):
        self.assert_rejected(snow_files({2: lambda data: data[:-100]}), "corrupt message")

    def test_icon_eu_bitmap_rejected(self):
        self.assert_rejected(snow_files({1: lambda data: mutate(data, 6, SECTION6_BITMAP, b"\x00")}),
                             "unsupported bitmap")

    def test_icon_eu_wrong_variable_behind_correct_file_name_rejected(self):
        total = raw_bytes(CYCLE, f"{PREFIX}single-level_{CYCLE}_000_TOT_PREC.grib2")
        self.assert_rejected(snow_files({0: lambda _: total}), r"parameter \(0, 1, 52\) is not \[0, 1, 55\]")

    def test_icon_eu_wrong_forecast_cycle_rejected(self):
        other = (f"{PREFIX}single-level_2026103103_000_TOT_PREC.grib2",
                 raw_bytes("2026103103", f"{PREFIX}single-level_2026103103_000_TOT_PREC.grib2"))
        found = self.violations(snow_files() + [other])
        self.assertTrue(any("forecast cycle 2026103103" in item for item in found), found)
        self.assertTrue(any("reference time 2026-10-31T03:00:00Z" in item for item in found), found)

    def test_icon_eu_cycle_mismatch_hidden_by_file_name_rejected(self):
        other = raw_bytes("2026103103", f"{PREFIX}single-level_2026103103_000_TOT_PREC.grib2")
        name = f"{PREFIX}single-level_{CYCLE}_000_TOT_PREC.grib2"
        self.assert_rejected(snow_files() + [(name, other)], "reference time 2026-10-31T03:00:00Z")

    def test_icon_eu_wrong_vertical_level_rejected(self):
        level10 = raw_bytes(CYCLE, f"{PREFIX}time-invariant_{CYCLE}_10_HHL.grib2")
        self.assert_rejected(snow_files() + [(f"{PREFIX}time-invariant_{CYCLE}_1_HHL.grib2", level10)],
                             "level 10 is not 1")

    def test_icon_eu_wrong_grid_rejected(self):
        self.assert_rejected(snow_files({1: lambda data: mutate(data, 3, SECTION3_NI, (1376).to_bytes(4, "big"))}),
                             "differs from contract grid")

    def test_icon_eu_incorrect_units_parameter_rejected(self):
        # 0-1-60 is snow depth water equivalent (kg m-2), not a snowfall rate accumulation.
        self.assert_rejected(snow_files({2: lambda data: mutate(data, 4, SECTION4_PARAMETER_NUMBER, b"\x3c")}),
                             r"parameter \(0, 1, 60\)")

    def test_icon_eu_wrong_time_unit_rejected(self):
        self.assert_rejected(snow_files({1: lambda data: mutate(data, 4, SECTION4_TIME_UNIT, b"\x01")}),
                             "time unit indicator 1 is not minutes")

    def test_icon_eu_insufficient_temporal_coverage_rejected(self):
        self.assert_rejected(snow_files(drop=2), r"insufficient temporal coverage, missing steps \[2\]")

    def test_icon_eu_unknown_accumulation_processing_rejected(self):
        self.assert_rejected(snow_files({3: lambda data: mutate(data, 4, SECTION4_STATISTICAL_PROCESSING, b"\xff")}),
                             "statistical processing 255 is not 1")

    def test_icon_eu_average_instead_of_accumulation_rejected(self):
        self.assert_rejected(snow_files({3: lambda data: mutate(data, 4, SECTION4_STATISTICAL_PROCESSING, b"\x00")}),
                             "statistical processing 0 is not 1")

    def test_icon_eu_accumulation_not_from_run_start_rejected(self):
        self.assert_rejected(snow_files({2: lambda data: mutate(data, 4, SECTION4_FORECAST_TIME, (60).to_bytes(4, "big"))}),
                             "statistics start 60 min after the cycle")

    def test_icon_eu_unknown_variable_rejected(self):
        name = f"{PREFIX}single-level_{CYCLE}_000_MH.grib2"
        self.assert_rejected(snow_files() + [(name, raw_bytes(CYCLE, SNOW_CON[0]))], "variable not in contract")

    def test_icon_eu_unknown_coverage_profile_rejected(self):
        with Sample(snow_files()) as directory:
            with self.assertRaises(audit.AuditError):
                audit.check_sample(directory, self.contract, "no-such-profile")


class IndependentReaderTest(unittest.TestCase):
    """The octet reader agrees with the committed decoded evidence."""

    @classmethod
    def setUpClass(cls):
        cls.rows = {row["path"]: row for row in load_json("decoded-evidence.json")["rows"]}

    def test_icon_eu_octets_match_decoded_evidence_for_every_raw_fixture(self):
        for cycle in sorted(os.listdir(RAW)):
            for name in sorted(os.listdir(os.path.join(RAW, cycle))):
                data = raw_bytes(cycle, name)
                row = self.rows[f"{cycle}/{name}"]
                self.assertEqual(hashlib.sha256(data).hexdigest(), row["grib2_sha256"], name)
                parsed = sections.parse(sections.split_messages(data)[0])
                product, rep = parsed["product"], parsed["representation"]
                self.assertEqual([parsed["discipline"], product["parameter_category"],
                                  product["parameter_number"]], row["parameter"], name)
                self.assertEqual([product["first_surface_type"], product["first_surface_scaled_value"]],
                                 row["first_surface"], name)
                self.assertEqual(rep["bits_per_value"], row["packing"][1], name)
                self.assertEqual(rep["binary_scale_factor"], row["packing"][2], name)

    def test_icon_eu_constant_hhl_values_decode_without_eccodes(self):
        name = f"{PREFIX}time-invariant_{CYCLE}_1_HHL.grib2"
        message = sections.split_messages(raw_bytes(CYCLE, name))[0]
        values = sections.decode_values(message, libaec=None)
        self.assertEqual(len(values), 1377 * 657)
        self.assertEqual(values[0], self.rows[f"{CYCLE}/{name}"]["values"]["max"])
        self.assertEqual(values[0], 22770.33203125)

    def test_icon_eu_sign_magnitude_decoding(self):
        self.assertEqual(sections.signed(0x8001, 2), -1)
        self.assertEqual(sections.signed(0x000B, 2), 11)

    def test_icon_eu_unsupported_template_rejected(self):
        message = sections.split_messages(raw_bytes(CYCLE, SNOW_CON[1]))[0]
        with self.assertRaises(sections.SectionError):
            sections.parse(mutate(message, 5, 9, (0).to_bytes(2, "big")))

    @unittest.skipIf(sections.load_libaec() is None, "libaec not installed (runs in the pinned container)")
    def test_icon_eu_libaec_values_match_evidence(self):
        import numpy  # noqa: PLC0415
        for name in SNOW_CON:
            message = sections.split_messages(raw_bytes(CYCLE, name))[0]
            values = numpy.asarray(sections.decode_values(message, sections.load_libaec()), dtype="<f8")
            digest = hashlib.sha256(values.tobytes()).hexdigest()
            self.assertEqual(digest, self.rows[f"{CYCLE}/{name}"]["values"]["values_sha256_f8le"], name)


def eccodes_available():
    try:
        audit.eccodes_module()
        return True
    except ImportError:
        return False


@unittest.skipUnless(eccodes_available(), "ecCodes Python bindings not installed")
class EccodesCrossCheckTest(unittest.TestCase):
    """ecCodes and the octet reader agree on every committed raw message."""

    def test_icon_eu_eccodes_agrees_with_octets_and_evidence(self):
        rows = {row["path"]: row for row in load_json("decoded-evidence.json")["rows"]}
        for cycle in sorted(os.listdir(RAW)):
            for name in sorted(os.listdir(os.path.join(RAW, cycle))):
                path = os.path.join(RAW, cycle, name)
                (keys, values), = list(audit.eccodes_messages(path))
                parsed = sections.parse(sections.split_messages(raw_bytes(cycle, name))[0])
                self.assertEqual(audit.cross_check(keys, parsed), [], name)
                self.assertEqual(audit.value_summary(values)["values_sha256_f8le"],
                                 rows[f"{cycle}/{name}"]["values"]["values_sha256_f8le"], name)


class ArtifactConsistencyTest(unittest.TestCase):
    """The contract, evidence, manifest and canonical schema stay mutually consistent."""

    @classmethod
    def setUpClass(cls):
        cls.contract = load_json("provider-contract.json")
        cls.evidence = load_json("decoded-evidence.json")
        cls.manifest = load_json("retrieval-manifest.json")
        with open(os.path.join(ROOT, "docs", "meteorology-contract.md"), encoding="utf-8") as handle:
            text = handle.read()
        block = text.split("<!-- BEGIN GENERATED FIELD SPEC MATRIX -->")[1].split("<!-- END GENERATED")[0]
        cls.canonical = re.findall(r"^\| `([a-z0-9_]+)` \|", block, flags=re.M)

    def test_icon_eu_matrix_covers_every_canonical_field_once(self):
        ids = [entry["canonical_id"] for entry in self.contract["field_matrix"]]
        self.assertEqual(len(self.canonical), 30)
        self.assertEqual(sorted(ids), sorted(self.canonical))
        self.assertEqual(len(ids), len(set(ids)))

    def test_icon_eu_classifications_are_explicit_and_evidenced(self):
        evidence_variables = {audit.describe(row["path"])["variable"] for row in self.evidence["rows"]}
        for entry in self.contract["field_matrix"]:
            name = entry["canonical_id"]
            self.assertIn(entry["classification"], CLASSIFICATIONS, name)
            self.assertIn(entry["quantity_mapping"], CLASSIFICATIONS, name)
            if entry["classification"] in SUPPORTED:
                self.assertTrue(entry["evidence"], name)
                self.assertEqual(entry["blocker_issues"], [], name)
                self.assertIn(entry["quantity_mapping"], SUPPORTED, name)
            else:
                self.assertTrue(entry["blocker_issues"], name)
            for reference in entry["evidence"]:
                if reference.startswith("rows:"):
                    self.assertIn(reference[5:], evidence_variables, (name, reference))
                else:
                    node = self.evidence
                    for part in reference.split("."):
                        node = node[part]

    def test_icon_eu_decision_matches_matrix(self):
        decision = self.contract["decision"]
        self.assertIn(decision["verdict"], decision["allowed_verdicts"])
        supported = sorted(entry["canonical_id"] for entry in self.contract["field_matrix"]
                           if entry["classification"] in SUPPORTED)
        self.assertEqual(sorted(decision["decodable_subset"]), supported)
        blocked = [consumer for consumer, item in self.contract["consumer_readiness"].items() if item["blocked_by"]]
        self.assertEqual(len(blocked), len(self.contract["consumer_readiness"]))
        if decision["verdict"] == "NO_GO":
            self.assertEqual(decision["decodable_subset_consumers"], [])

    def test_icon_eu_message_identities_match_decoded_rows(self):
        identities = self.contract["message_identities"]
        checked = 0
        for row in self.evidence["rows"]:
            info = audit.describe(row["path"])
            expected = identities.get(info["variable"])
            if expected is None or info["kind"] == "pressure-level" or info["cycle"] != CYCLE:
                continue
            self.assertEqual(row["parameter"], expected["parameter"], row["path"])
            self.assertEqual(row["product_template"], expected["product_template"], row["path"])
            self.assertEqual(row["first_surface"][0], expected["first_surface_type"], row["path"])
            self.assertEqual(row["second_surface_type"], expected["second_surface_type"], row["path"])
            if "statistical_processing" in expected:
                self.assertEqual(row["statistics"][0], expected["statistical_processing"], row["path"])
            checked += 1
        self.assertGreater(checked, 200)

    def test_icon_eu_raw_fixtures_are_unmodified_manifest_bytes(self):
        hashes = {item["path"][: -len(".bz2")]: item["grib2_sha256"] for item in self.manifest["files"]}
        count = 0
        for cycle in sorted(os.listdir(RAW)):
            for name in sorted(os.listdir(os.path.join(RAW, cycle))):
                self.assertEqual(hashlib.sha256(raw_bytes(cycle, name)).hexdigest(), hashes[f"{cycle}/{name}"])
                count += 1
        self.assertEqual(count, 10)
        self.assertEqual(len(self.manifest["files"]), self.evidence["file_count"])

    def test_icon_eu_sample_request_expands_to_manifest(self):
        with open(os.path.join(FIXTURE, "sample-request.json"), encoding="utf-8") as handle:
            request = json.load(handle)
        expanded = {path.replace(os.sep, "/") for _, path in fetch.expand_request(request)}
        self.assertEqual(expanded, {item["path"] for item in self.manifest["files"]})

    def test_icon_eu_evidence_records_two_identical_decoders(self):
        self.assertTrue(all(row["values_identical_across_decoders"] for row in self.evidence["rows"]))
        self.assertTrue(all(row["independent_max_abs_difference"] == 0.0 for row in self.evidence["rows"]))
        versions = {self.evidence["decoders"][role]["eccodes_api_version"] for role in ("primary", "secondary")}
        self.assertEqual(versions, {"2.24.2", "2.48.0"})


if __name__ == "__main__":
    unittest.main()
