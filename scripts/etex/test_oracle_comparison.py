"""Checks for ETEX observation timing and paired-output validation."""

import json
import struct
import tempfile
import unittest
from datetime import datetime, timedelta
from pathlib import Path

import numpy as np

from compare_oracle_observations import Window, compare, gpu_windows, station_mean
from parse_measurements import parse_measurements


class EtexComparisonTest(unittest.TestCase):
    def test_end_to_end_pair_uses_both_model_fields(self):
        def record(values, kind):
            payload = struct.pack("<" + str(len(values)) + kind, *values)
            marker = struct.pack("<i", len(payload))
            return marker + payload + marker

        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            fortran = root / "fortran"
            fortran.mkdir()
            (fortran / "header_txt").write_text(
                "outlon0, outlat0, numxgrid, numygrid, dxout, dyout\n"
                "-2 48 2 2 1 1\n"
                "numzgrid, outheight\n1 100\n"
                "interval, averaging, sampling\n10800 10800 900\n")
            (fortran / "dates").write_text("19941023190000\n")
            binary = (record([10800], "i")
                      + record([0], "i") + record([], "i")
                      + record([0], "i") + record([], "f")
                      + record([0], "i") + record([], "i")
                      + record([0], "i") + record([], "f")
                      + record([1], "i") + record([4], "i")
                      + record([1], "i") + record([100.0], "f"))
            (fortran / "grid_conc_19941023190000_001").write_bytes(binary)
            measurements = root / "observations.json"
            measurements.write_text(json.dumps({"measurements": [{
                "station": 1, "start_time": "1994-10-23 16:00",
                "end_time": "1994-10-23 19:00", "lon": -1.5, "lat": 48.5,
                "concentration_pg_m3": 25.0}]}))
            gpu = root / "gpu.json"
            gpu.write_text(json.dumps({
                "grid": {"nx": 2, "ny": 2, "nz": 1, "dx": 1.0, "dy": 1.0,
                         "xlon0": -2.0, "ylat0": 48.0, "heights_m": [100.0]},
                "averaging_seconds": 10800, "sampling_seconds": 900,
                "timesteps": [{"window_start_epoch_seconds": 782928000,
                               "epoch_seconds": 782938800, "samples": 13,
                               "concentration_mass_kg": [1.0] * 4}]}))
            gpu_log = root / "gpu.log"
            gpu_log.write_text("[INFO] wgpu adapter: WARP (Dx12, Cpu)\n")
            fortran_log = root / "fortran.log"
            fortran_log.write_text("CONGRATULATIONS\n")
            result = compare(measurements, gpu, fortran,
                             gpu_log=gpu_log, fortran_log=fortran_log)
            self.assertEqual(result["paired_observation_count"], 1)
            self.assertEqual(result["fortran_vs_observations"]["bias_pg_m3"], 0.0)
            self.assertNotEqual(result["gpu_vs_observations"]["bias_pg_m3"], 0.0)
            self.assertEqual(len(result["input_sha256"]["fortran_concentration_files"]), 1)
            self.assertIn("WARP", result["gpu_adapter"])

    def test_datem_duration_is_hhmm(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "measurements.txt"
            path.write_text("1994 10 23 1600 0300 48.0 -2.0 25.0 1\n")
            record, = parse_measurements(str(path))
            self.assertEqual(record["duration_min"], 180)
            self.assertEqual(record["end_time"], "1994-10-23 19:00")

    def test_station_mean_weights_complete_windows(self):
        start = datetime(1994, 10, 23, 16)
        grid = {"xlon0": -2.0, "ylat0": 48.0, "dx": 1.0, "dy": 1.0,
                "nx": 2, "ny": 2}
        windows = [Window(start, start + timedelta(hours=3), np.full((2, 2), 10.0)),
                   Window(start + timedelta(hours=3), start + timedelta(hours=6),
                          np.full((2, 2), 30.0))]
        self.assertEqual(station_mean(windows, grid, -1.5, 48.5,
                                      start + timedelta(hours=1),
                                      start + timedelta(hours=5)), 20.0)
        self.assertIsNone(station_mean(windows[:1], grid, -1.5, 48.5,
                                       start, start + timedelta(hours=6)))

    def test_gpu_output_rejects_incomplete_window_and_field(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "gpu.json"
            output = {
                "grid": {"nx": 2, "ny": 2, "nz": 1, "dx": 1.0, "dy": 1.0,
                         "xlon0": -2.0, "ylat0": 48.0, "heights_m": [100.0]},
                "averaging_seconds": 10800,
                "sampling_seconds": 900,
                "timesteps": [{"window_start_epoch_seconds": 0,
                               "epoch_seconds": 10800, "samples": 11,
                               "concentration_mass_kg": [0.0] * 4}],
            }
            path.write_text(json.dumps(output))
            with self.assertRaisesRegex(ValueError, "incomplete"):
                gpu_windows(path)
            output["timesteps"][0]["samples"] = 13
            output["timesteps"][0]["concentration_mass_kg"] = [0.0] * 3
            path.write_text(json.dumps(output))
            with self.assertRaisesRegex(ValueError, "wrong shape"):
                gpu_windows(path)


if __name__ == "__main__":
    unittest.main()
