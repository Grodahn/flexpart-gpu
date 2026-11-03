#!/usr/bin/env python3
"""Self-tests for the #232 ICON-EU precipitation admission decision.

The always-run tests use the committed decision, evidence and raw GRIB2 bytes
with the independent octet reader only. The two-decoder re-decode of the
committed raw subset (ecCodes plus libaec) runs when both libraries are
installed, as in the pinned flexpart-fortran container, and is skipped otherwise.

Run: python -I scripts/provider/test_icon_eu_precipitation_decision.py
"""

import json
import math
import os
import shutil
import sys
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, HERE)
import icon_eu_grib2_sections as sections  # noqa: E402
import icon_eu_precipitation_decision as decision  # noqa: E402

FIXTURE = os.path.join(ROOT, "fixtures", "meteorology", "icon-eu-precipitation-232")
DECISION = os.path.join(FIXTURE, "decision-v1.json")
EVIDENCE = os.path.join(FIXTURE, "evidence-v1.json")
AUDIT = os.path.join(ROOT, "fixtures", "meteorology", "icon-eu-audit-v1")
MANIFEST = os.path.join(AUDIT, "retrieval-manifest.json")
# Committed raw roots; #230 already holds SNOW_CON 0-3 and the step-0 TOT_PREC/RAIN_GSP anchors.
RAW_ROOTS = (os.path.join(FIXTURE, "raw"), os.path.join(AUDIT, "raw"))
COMMITTED_SERIES = ("SNOW_CON", "SNOW_GSP")
COMMITTED_STEPS = (0, 1, 2, 3, 47, 48)


def load(path):
    with open(path, encoding="utf-8") as handle:
        return json.load(handle)


def restore_nan(payload):
    """Undo the artifact's "NaN" string encoding of non-finite control values."""
    if payload == "NaN":
        return math.nan
    if isinstance(payload, dict):
        return {key: restore_nan(value) for key, value in payload.items()}
    if isinstance(payload, list):
        return [restore_nan(value) for value in payload]
    return payload


def committed_path(variable, step):
    name = decision.FILE_TEMPLATE.format(cycle=decision.CYCLE, step=step, variable=variable)
    for root in RAW_ROOTS:
        path = os.path.join(root, decision.CYCLE, name)
        if os.path.exists(path):
            return path
    return None


def evidence_message(evidence, variable, step):
    return next(m for m in evidence["messages"] if m["variable"] == variable and m["step"] == step)


def read(path):
    with open(path, "rb") as handle:
        return handle.read()


class DecisionPolicyTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.decision = restore_nan(load(DECISION))
        cls.evidence = load(EVIDENCE)

    def test_icon_precip_controls_reproduce_expected_verdicts(self):
        for control in self.decision["controls"]:
            with self.subTest(control=control["name"]):
                verdict, code, bound = decision.classify_pair(
                    control["field"], control["previous"], control["current"])
                self.assertEqual((verdict, code), (control["expected_verdict"], control["expected_code"]))
                self.assertEqual(bound, control["bound_kg_m2"])

    def test_icon_precip_genuine_rows_are_admitted_bounded(self):
        self.assertEqual(len(self.decision["genuine_rows"]), 8)
        for row in self.decision["genuine_rows"]:
            with self.subTest(row=row["name"]):
                verdict, code, _ = decision.classify_pair(row["field"], row["previous"], row["current"])
                self.assertEqual((verdict, code), ("ADMIT_BOUNDED", "quantization_compatible_decrease"))

    def test_icon_precip_controls_cover_every_code(self):
        codes = {control["expected_code"] for control in self.decision["controls"]}
        self.assertEqual(len(codes), 21)
        self.assertTrue({"ADMIT_BOUNDED", "REJECT", "UNRESOLVED", "OUTSIDE_RULE"}
                        <= {control["expected_verdict"] for control in self.decision["controls"]})

    def test_icon_precip_build_reproduces_committed_decision(self):
        with tempfile.TemporaryDirectory() as directory:
            copy = os.path.join(directory, "decision.json")
            shutil.copyfile(DECISION, copy)
            self.assertEqual(decision.main(["build", "--evidence", EVIDENCE, "--decision", copy]), 0)
            with open(DECISION, "rb") as committed, open(copy, "rb") as rebuilt:
                self.assertEqual(committed.read().replace(b"\r\n", b"\n"), rebuilt.read())

    def test_icon_precip_step_admission_follows_closure(self):
        admission = self.decision["sample_step_admission"]
        for entry in self.evidence["closure"]:
            expected = "ADMIT" if entry["cells_beyond_bound"] == 0 else "REJECT"
            self.assertEqual(admission[str(entry["step"])], expected)
            self.assertEqual(entry["cells_beyond_bound_positive"], 0, "no unpublished component")
        self.assertEqual([s for s, v in admission.items() if v == "REJECT"], ["78", "81", "84"])

    def test_icon_precip_evidence_decreases_stay_inside_bound(self):
        for pair in self.evidence["component_pairs"]:
            with self.subTest(pair=(pair["variable"], pair["from_step"], pair["to_step"])):
                if pair["identical_packing"]:
                    self.assertEqual(pair["decreasing_cells"], 0)
                elif pair["decreasing_cells"]:
                    self.assertLess(-pair["most_negative_delta_kg_m2"], pair["source_bound_kg_m2"])
                    self.assertEqual(-pair["most_negative_delta_kg_m2"],
                                     pair["round_to_nearest_nested_bound_kg_m2"])
        for pair in self.evidence["canonical_pairs"]:
            self.assertEqual(pair["violating_cells"], 0)
            self.assertLess(pair["max_decrease_to_bound_ratio"], 1.0)


class RawFixtureTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.evidence = load(EVIDENCE)

    def committed(self):
        for variable in COMMITTED_SERIES:
            for step in COMMITTED_STEPS:
                path = committed_path(variable, step)
                self.assertIsNotNone(path, f"{variable} step {step} is not committed")
                yield variable, step, path

    def test_icon_precip_raw_octets_match_evidence(self):
        for variable, step, path in self.committed():
            with self.subTest(variable=variable, step=step):
                message = read(path)
                expected = evidence_message(self.evidence, variable, step)
                digest = decision.sha256_bytes(message)
                self.assertEqual(digest, expected["packing"]["message_sha256"])
                self.assertEqual(decision.packing_record(sections.parse(message), digest), expected["packing"])

    def test_icon_precip_mutated_sample_fails_closed(self):
        source = committed_path("TOT_PREC", 0)
        with tempfile.TemporaryDirectory() as directory:
            os.makedirs(os.path.join(directory, decision.CYCLE))
            mutated = bytearray(read(source))
            mutated[-5] ^= 0x01
            with open(os.path.join(directory, decision.CYCLE, os.path.basename(source)), "wb") as handle:
                handle.write(mutated)
            with self.assertRaisesRegex(decision.EvidenceError, "pinned retrieval manifest"):
                decision.read_messages(directory, decision.load_manifest(MANIFEST), {}, None, None, None)

    def test_icon_precip_two_decoders_reproduce_committed_evidence(self):
        try:
            import eccodes
            import numpy
        except ImportError:
            self.skipTest("ecCodes/numpy not installed")
        libaec = sections.load_libaec()
        if libaec is None:
            self.skipTest("libaec not installed")
        decoded = {}
        for variable, step, path in self.committed():
            message = read(path)
            parsed = sections.parse(message)
            record = decision.packing_record(parsed, decision.sha256_bytes(message))
            integers = decision.decode_integers(message, libaec, numpy)
            values, _ = decision.eccodes_values(message, eccodes, numpy)
            independent = record["referenceValue"] + integers * (decision.quantum(record) or 1.0)
            self.assertTrue(numpy.array_equal(values, independent), f"{variable} {step}")
            expected = evidence_message(self.evidence, variable, step)
            self.assertEqual(decision.sha256_bytes(values.astype("<f8").tobytes()), expected["values_sha256_f8le"])
            decoded[(variable, step)] = {"record": record, "values": values, "integers": integers,
                                         "summary": {"variable": variable, "step": step}}
        for variable in COMMITTED_SERIES:
            for before, after in zip(COMMITTED_STEPS, COMMITTED_STEPS[1:]):
                recomputed = decision.component_pair(numpy, decoded[(variable, before)], decoded[(variable, after)])
                frozen = next(p for p in self.evidence["component_pairs"]
                              if (p["variable"], p["from_step"], p["to_step"]) == (variable, before, after))
                self.assertEqual(recomputed, frozen)


if __name__ == "__main__":
    unittest.main()
