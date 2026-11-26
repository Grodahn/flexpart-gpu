"""Negative controls for the ERA5/ETEX compatibility audit, using the real NPZ."""

from io import BytesIO
import json
import unittest

import numpy as np

from check_era5_etex_native_values import EVIDENCE, RAW, check_surface_content, grib2_frames


class SurfaceAuditTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        request = json.loads((RAW / "surface-request.json").read_bytes())
        evidence = json.loads(EVIDENCE.read_bytes())
        cls.model_epochs = sorted(
            int(epoch) for source in evidence["sources"] for epoch in source["volumes_by_epoch"]
        )
        cls.times = request["times"]
        with np.load(BytesIO((RAW / request["file"]).read_bytes()), allow_pickle=False) as archive:
            cls.surface = {
                key: archive[key] for key in ("times", "latitudes", "longitudes", "surface_pressure")
            }

    def test_real_archive_passes_content_checks(self):
        check_surface_content(self.surface, self.times, self.model_epochs)

    def test_surface_and_request_with_wrong_time_fail_against_model(self):
        times = [*self.times]
        times[0] = "1994-10-23T16:00:00"
        surface = {**self.surface, "times": np.asarray(times, dtype="S19")}
        with self.assertRaisesRegex(ValueError, "differ from GRIB valid times"):
            check_surface_content(surface, times, self.model_epochs)

    def test_reversed_latitude_order_fails(self):
        surface = {**self.surface, "latitudes": self.surface["latitudes"][::-1]}
        with self.assertRaises(AssertionError):
            check_surface_content(surface, self.times, self.model_epochs)

    def test_missing_surface_pressure_pixel_fails(self):
        pressure = self.surface["surface_pressure"].copy()
        pressure[0, 0, 0] = np.nan
        surface = {**self.surface, "surface_pressure": pressure}
        with self.assertRaisesRegex(ValueError, "invalid ARCO surface pressure array"):
            check_surface_content(surface, self.times, self.model_epochs)


class GribAuditTests(unittest.TestCase):
    def test_truncated_message_fails(self):
        request = json.loads((RAW / "request.json").read_bytes())
        with (RAW / request["file"]).open("rb") as source:
            header = source.read(16)
            size = int.from_bytes(header[8:], "big")
            frame = header + source.read(size - len(header))
        self.assertEqual(list(grib2_frames(frame)), [frame])
        with self.assertRaisesRegex(ValueError, "invalid GRIB2 frame"):
            list(grib2_frames(frame[:-1]))


if __name__ == "__main__":
    unittest.main()
