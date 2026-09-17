"""Regression tests for comparable FLEXPART and GPU output-grid quantities."""

import unittest

import numpy as np

from compare_concentrations import (
    concentration_to_cell_mass,
    compute_center_of_mass,
    validate_shared_grid,
)


class ConcentrationComparisonTests(unittest.TestCase):
    def test_concentration_is_weighted_by_layer_volume_and_latitude(self):
        header = {
            "numxgrid": 1,
            "numygrid": 2,
            "numzgrid": 2,
            "outlon0": 0.0,
            "outlat0": 0.0,
            "dxout": 1.0,
            "dyout": 30.0,
            "outheights": [100.0, 500.0],
        }
        concentration = np.ones((1, 2, 2), dtype=np.float32)
        mass = concentration_to_cell_mass(concentration, header)

        # Equal concentration in a 400 m layer represents four times the
        # mass of a 100 m layer at the same latitude.
        self.assertAlmostEqual(mass[0, 0, 1] / mass[0, 0, 0], 4.0)
        # Spherical cell area decreases towards higher latitude.
        expected_area_ratio = (np.sin(np.deg2rad(60)) - np.sin(np.deg2rad(30))) / np.sin(np.deg2rad(30))
        self.assertAlmostEqual(mass[0, 1, 0] / mass[0, 0, 0], expected_area_ratio)
        self.assertFalse(np.allclose(mass / mass.sum(), concentration / concentration.sum()))

    def test_vertical_center_uses_mass_and_layer_midpoints(self):
        mass = np.array([[[100.0, 800.0]]])
        center = compute_center_of_mass(mass, 0.0, 0.0, 1.0, 1.0, [100.0, 500.0])
        self.assertAlmostEqual(center["z_m"], (100.0 * 50.0 + 800.0 * 300.0) / 900.0)

    def test_mismatched_grid_is_rejected(self):
        header = {"numxgrid": 1, "numygrid": 1, "numzgrid": 2,
                  "outlon0": 0.0, "outlat0": 0.0, "dxout": 1.0,
                  "dyout": 1.0, "outheights": [100.0, 500.0]}
        gpu_grid = {"nx": 1, "ny": 1, "nz": 2, "xlon0": 0.0,
                    "ylat0": 0.0, "dx": 1.0, "dy": 1.0,
                    "heights_m": [100.0, 400.0]}
        with self.assertRaisesRegex(ValueError, "vertical layer boundaries"):
            validate_shared_grid(header, gpu_grid)

    def test_equator_crossing_uses_fortran_meridional_arc(self):
        header = {"numxgrid": 1, "numygrid": 1, "numzgrid": 1,
                  "outlat0": -1.0, "dxout": 2.0, "dyout": 2.0,
                  "outheights": [100.0]}
        mass = concentration_to_cell_mass(np.ones((1, 1, 1)), header)
        expected = (6_371_000.0 ** 2 * np.deg2rad(2.0) ** 2 * 100.0)
        self.assertAlmostEqual(mass[0, 0, 0] / expected, 1.0)


if __name__ == "__main__":
    unittest.main()
