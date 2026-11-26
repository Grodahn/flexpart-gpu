#!/usr/bin/env python3
"""Offline locks for the #235 decision, source bytes and original transcript."""
import bisect
import math
import sys
import tempfile
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import icon_eu_precipitation_time as replay


DECISION = replay.read_json(replay.FIXTURE / "decision.json")
EVIDENCE = replay.read_json(replay.FIXTURE / "evidence.json")


def support(segment, start, end, endpoints, fields):
    """Independently derive required lead hours from a declared regular segment."""
    delta = segment["delta"]
    published = list(range(segment["start"], segment["end"] + 1, delta))
    members = published[2:-2]
    if len(members) < 2 or not members[0] <= start <= end <= members[-1]:
        raise ValueError("insufficient accumulation halo")
    first = bisect.bisect_right(members, start) - 1
    last = min(bisect.bisect_right(members, end), len(members) - 1)
    required_members = members[first:last + 1]
    required = list(range(required_members[0] - 2 * delta, required_members[-1] + 2 * delta + 1, delta))
    if len({run for _, run in endpoints}) != 1:
        raise ValueError("different forecast run")
    times = [time for time, _ in endpoints]
    if times != sorted(set(times)):
        raise ValueError("duplicate or out-of-order endpoint")
    if any(time not in published for time in times):
        raise ValueError("invalid forecast interval")
    if not set(required) <= set(times):
        raise ValueError("missing expected endpoint")
    if not set(required_members) <= set(fields):
        raise ValueError("missing meteorological bracket")
    return required_members, required


class DecisionTests(unittest.TestCase):
    def test_verdict_and_locked_provenance(self):
        self.assertEqual(DECISION["version"], 1)
        self.assertEqual(DECISION["verdict"], "REQUIRES_NEW_ORACLE")
        self.assertEqual(DECISION["runtime_admission"], "REJECT")
        self.assertFalse(DECISION["legacy_diagnostic"]["conserves_interval_amount"])
        for name in ("flexpart", "flex_extract"):
            self.assertEqual(DECISION["legacy_diagnostic"]["reference_pins"][name],
                             EVIDENCE[f"{name}_revision"])
        for path, expected in DECISION["input_sha256"].items():
            self.assertEqual(replay.digest(replay.ROOT / path), expected, path)

    def test_raw_bytes_and_true_accumulation_intervals(self):
        hashes = {}
        for entry, data in replay.raw_messages():
            record = replay.admission.packing_record(data)
            evidence = next(row for row in EVIDENCE["messages"] if row["path"] == entry["path"])
            self.assertEqual(replay.admission.PARAMETER_NUMBERS[evidence["variable"]], record["parameterNumber"])
            self.assertEqual(record["typeOfStatisticalProcessing"], 1)
            self.assertEqual(record["productDefinitionTemplateNumber"], 8)
            self.assertEqual(record["forecastTime"], 0)
            self.assertEqual(record["indicatorOfUnitForTimeRange"], 0)
            self.assertEqual(record["forecast_reference_s"], evidence["start_s"])
            self.assertEqual(record["end_of_interval_s"], evidence["end_s"])
            self.assertEqual(record["lengthOfTimeRange"], 60 * evidence["step"])
            self.assertEqual(evidence["end_s"] - evidence["start_s"], 3600 * evidence["step"])
            self.assertEqual(entry["grib2_sha256"], evidence["message_sha256"])
            hashes[(evidence["variable"], evidence["step"])] = record["message_sha256"]
        self.assertEqual(set(hashes), {(v, s) for v in replay.admission.VARIABLES for s in replay.STEPS})
        self.assertEqual(len(set(hashes.values())), 30)
        self.assertTrue(all(step["closure_verified"] is True for step in EVIDENCE["step_closure"]))

    def test_original_output_and_amount_comparisons(self):
        queries = 0
        self.assertEqual([case["id"] for case in EVIDENCE["cases"]], DECISION["evidence"]["case_ids"])
        for case in EVIDENCE["cases"]:
            with self.subTest(case=case["id"]):
                for kind in ("input", "output"):
                    self.assertEqual(replay.admission.sha256_bytes(case[f"raw_{kind}"].encode()),
                                     case[f"{kind}_sha256"])
                self.assertEqual(replay.driver_input(case), case["raw_input"])
                self.assertEqual(replay.parse_output(case["raw_output"]), case["rows"])
                self.assertEqual(replay.check_rows(case, case["rows"]), case["max_candidate_error_mm_h"])
                queries += len(case["rows"])
        self.assertEqual(queries, DECISION["evidence"]["query_count"])
        self.assertEqual(queries * 2, DECISION["evidence"]["precipitation_comparison_count"])
        for pair, final, field in ((0, 1, "lsp"), (2, 3, "cp")):
            case = EVIDENCE["cases"][pair]
            end = EVIDENCE["cases"][final]
            for ordinary, duplicate in zip(case["rows"][4::5], end["rows"]):
                self.assertEqual(ordinary["query"], duplicate["query"])
                for component in ("lsp", "cp"):
                    self.assertTrue(math.isclose(ordinary[component], duplicate[component], rel_tol=1e-6))
            source_amount = case["interval_rates_mm_h"][field][2][4]
            original_integral = (case["rows"][0][field] + case["rows"][4][field]) / 2
            self.assertEqual(case["center_amount_comparison_mm"][field],
                             {"icon_interval_amount": source_amount, "integral_of_original_rates": original_integral})
            self.assertGreater(original_integral, 2 * source_amount)
            self.assertGreater(EVIDENCE["member_statistics"][field]["dry_middle_interval_cells_with_nonzero_linear_amount"], 0)

    def test_missing_or_corrupt_temporal_terms_fail(self):
        import copy

        case = EVIDENCE["cases"][0]
        rows = copy.deepcopy(case["rows"])
        rows[-1]["dts"][2] *= 3
        with self.assertRaisesRegex(ValueError, "temporal weights"):
            replay.check_rows(case, rows)
        with self.assertRaisesRegex(ValueError, "mismatched oracle query"):
            replay.check_rows(case, rows[:-1])

    def test_replay_rejects_changed_or_unrecorded_driver_bytes(self):
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp)
            log = directory / "hashes.txt"
            for kind, name in (("input", "rain-lsp-pair.txt"), ("output", "rain-lsp-pair-output.txt")):
                path = directory / name
                case = EVIDENCE["cases"][0]
                data = case[f"raw_{kind}"].encode()
                line = f"{case[f'{kind}_sha256']}  /container mount/{name}\n"
                path.write_bytes(data)
                log.write_text(line)
                checksums = replay.replay_hashes(log)
                self.assertEqual(replay.checked_replay_bytes(path, checksums), data)
                with self.assertRaisesRegex(ValueError, "missing or mismatched original harness checksum"):
                    replay.checked_replay_bytes(path, {})
                path.write_bytes(data + b"\n")
                with self.assertRaisesRegex(ValueError, "missing or mismatched original harness checksum"):
                    replay.checked_replay_bytes(path, checksums)
                log.write_text(line * 2)
                with self.assertRaisesRegex(ValueError, "duplicate original harness checksum"):
                    replay.replay_hashes(log)

    def test_edge_brackets(self):
        hourly = DECISION["coverage"]["schedules"]["main"]["segments"][0]
        endpoints = [(t, "cycle") for t in range(79)]
        self.assertEqual(support(hourly, 2, 50, endpoints, range(2, 52))[1], list(range(54)))
        self.assertEqual(support(hourly, 2.25, 49.25, endpoints, range(2, 51))[1], list(range(53)))
        self.assertEqual(support(hourly, 75, 76, endpoints, [75, 76]), ([75, 76], list(range(73, 79))))
        with self.assertRaisesRegex(ValueError, "insufficient accumulation halo"):
            support(hourly, 0, 48, endpoints, range(49))
        with self.assertRaisesRegex(ValueError, "missing meteorological bracket"):
            support(hourly, 2, 2.25, endpoints, [2])
        with self.assertRaisesRegex(ValueError, "missing expected endpoint"):
            support(hourly, 2, 2.25, endpoints[1:], [2, 3])

    def test_missing_hours_and_cycle_splicing(self):
        hourly = DECISION["coverage"]["schedules"]["main"]["segments"][0]
        with self.assertRaisesRegex(ValueError, "missing expected endpoint"):
            support(hourly, 2, 2.5, [(t, "a") for t in (0, 1, 2, 4, 5)], [2, 3])
        with self.assertRaisesRegex(ValueError, "different forecast run"):
            support(hourly, 2, 2.5, [(t, "a" if t < 3 else "b") for t in range(6)], [2, 3])
        with self.assertRaisesRegex(ValueError, "duplicate or out-of-order"):
            support(hourly, 2, 2.5, [(t, "a") for t in (0, 1, 2, 3, 3, 4, 5)], [2, 3])

    def test_coarse_transition_and_frozen_232_closure(self):
        coverage = DECISION["coverage"]
        self.assertFalse(coverage["bridge_segments"])
        for schedule in coverage["schedules"].values():
            for segment in schedule["segments"]:
                delta = segment["delta"]
                possible = list(range(segment["start"], segment["end"] + 1, delta))[2:-2]
                self.assertEqual((possible[0], possible[-1]) if possible else (None, None),
                                 (segment["first_member"], segment["last_member"]))
        hourly, coarse = coverage["schedules"]["main"]["segments"]
        endpoints = [(t, "a") for t in range(78, 121, 3)]
        self.assertEqual(support(coarse, 84, 85, endpoints, [84, 87])[1], [78, 81, 84, 87, 90, 93])
        with self.assertRaisesRegex(ValueError, "invalid forecast interval"):
            support(coarse, 84, 85, sorted(endpoints + [(82, "a")]), [84, 87])
        for segment in (hourly, coarse):
            with self.assertRaisesRegex(ValueError, "insufficient accumulation halo"):
                support(segment, 75, 85, endpoints, [75, 85])
        evidence = replay.read_json(replay.ROOT / DECISION["evidence"]["transition_reference"])
        self.assertEqual(evidence["cycle"], DECISION["evidence"]["transition_cycle"])
        self.assertEqual([r["step"] for r in evidence["closure"] if not r["closure_verified"]],
                         DECISION["evidence"]["transition_leads_rejected_by_232"])


if __name__ == "__main__":
    unittest.main()
