"""Analytical unit tests for the canonical evaluation metrics.

Each test uses a small hand-computed field or particle distribution with a
known answer. Run from the repository root with:

    python scripts/evaluate/test_metrics.py
"""

import math
import sys
import unittest
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

import metrics
import io_gpu


class PearsonCorrelationTest(unittest.TestCase):
    def test_perfect_correlation(self):
        self.assertAlmostEqual(metrics.pearson_correlation([1, 2, 3], [2, 4, 6]), 1.0)

    def test_zero_variance_returns_none(self):
        self.assertIsNone(metrics.pearson_correlation([1, 1, 1], [1, 2, 3]))
        self.assertIsNone(metrics.pearson_correlation([1, 2, 3], [5, 5, 5]))

    def test_empty_or_mismatched_raises(self):
        with self.assertRaises(ValueError):
            metrics.pearson_correlation([], [])
        with self.assertRaises(ValueError):
            metrics.pearson_correlation([1, 2], [1])


class FieldComparisonTest(unittest.TestCase):
    def test_identical_fields(self):
        result = metrics.field_comparison_metrics([1, 2, 3], [1, 2, 3], "same")
        self.assertEqual(result["bias"], 0.0)
        self.assertEqual(result["rmse"], 0.0)
        self.assertEqual(result["normalized_rmse"], 0.0)
        self.assertAlmostEqual(result["correlation"], 1.0)

    def test_known_bias_and_rmse(self):
        # obs=[0,2], mod=[2,2]: diff=[2,0], bias=1, rmse=sqrt(2).
        result = metrics.field_comparison_metrics([0, 2], [2, 2])
        self.assertAlmostEqual(result["bias"], 1.0)
        self.assertAlmostEqual(result["rmse"], math.sqrt(2.0))
        # denom = max(mean|obs|=1, mean|mod|=2) = 2.
        self.assertAlmostEqual(result["normalized_rmse"], math.sqrt(2.0) / 2.0)

    def test_all_zero_fields_give_none_nrmse(self):
        result = metrics.field_comparison_metrics([0, 0], [0, 0])
        self.assertIsNone(result["normalized_rmse"])
        self.assertIsNone(result["correlation"])


class MassBudgetTest(unittest.TestCase):
    def test_closed_budget(self):
        budget = metrics.mass_budget(1.0, 0.7, dry_deposited_kg=0.2, wet_deposited_kg=0.1)
        self.assertAlmostEqual(budget["accounted_mass_kg"], 1.0)
        self.assertAlmostEqual(budget["absolute_error_kg"], 0.0)
        self.assertAlmostEqual(budget["relative_error"], 0.0)

    def test_open_budget_reports_error(self):
        budget = metrics.mass_budget(1.0, 0.9)
        self.assertAlmostEqual(budget["absolute_error_kg"], -0.1)
        self.assertAlmostEqual(budget["relative_error"], -0.1)

    def test_negative_mass_is_rejected(self):
        with self.assertRaises(ValueError):
            metrics.mass_budget(1.0, -0.1)


class CenterOfMassTest(unittest.TestCase):
    def test_single_cell_grid(self):
        com = metrics.center_of_mass_grid([0, 0, 5, 0], 2, 2, 1,
                                           10.0, 20.0, 1.0, 1.0, [100.0])
        self.assertIsNotNone(com)
        # Active cell is ix=1, iy=0 -> lon=10+1.5=11.5, lat=20+0.5=20.5.
        self.assertAlmostEqual(com["lon_deg"], 11.5)
        self.assertAlmostEqual(com["lat_deg"], 20.5)
        self.assertAlmostEqual(com["z_m"], 50.0)

    def test_unequal_layers_use_mass_and_midpoints(self):
        concentration = [1.0, 1.0]
        mass = io_gpu.concentration_to_mass_per_cell(
            concentration, 1, 1, 2, 10.0, 1.0, 1.0, [100.0, 500.0])
        self.assertAlmostEqual(mass[1] / mass[0], 4.0)
        com = metrics.center_of_mass_grid(
            mass, 1, 1, 2, 0.0, 10.0, 1.0, 1.0, [100.0, 500.0])
        self.assertAlmostEqual(com["z_m"], 250.0)

    def test_equator_cell_volume_matches_fortran_arc(self):
        expected = io_gpu.EARTH_RADIUS_M ** 2 * math.radians(2.0) ** 2 * 100.0
        self.assertAlmostEqual(io_gpu.cell_volume_m3(0.0, 2.0, 2.0, 100.0), expected)

    def test_empty_grid_returns_none(self):
        self.assertIsNone(metrics.center_of_mass_grid([0, 0, 0], 3, 1, 1,
                                                      0, 0, 1, 1, [50]))

    def test_particle_com_is_mass_weighted(self):
        com = metrics.center_of_mass_particles([0, 10], [0, 0], [0, 0], [1, 3])
        self.assertAlmostEqual(com["lon_deg"], 7.5)
        self.assertAlmostEqual(com["total_mass_kg"], 4.0)


class HorizontalCovarianceTest(unittest.TestCase):
    def test_axis_aligned_square(self):
        # Four unit masses at (+/-1 deg lon, 0) and (0, +/-1 deg lat)
        # around the equator: variances are (km-per-deg^2)/2 per axis.
        lons = [1, -1, 0, 0]
        lats = [0, 0, 1, -1]
        cov = metrics.horizontal_covariance(lons, lats, [1, 1, 1, 1])
        expected = (metrics.DEG_TO_KM ** 2) / 2.0
        self.assertAlmostEqual(cov["lon_mean_deg"], 0.0)
        self.assertAlmostEqual(cov["covariance_km2"][0][0], expected, places=6)
        self.assertAlmostEqual(cov["covariance_km2"][1][1], expected, places=6)
        self.assertAlmostEqual(cov["covariance_km2"][0][1], 0.0, places=9)
        self.assertAlmostEqual(cov["eigenvalues_km2"][0], expected, places=6)
        self.assertAlmostEqual(cov["eigenvalues_km2"][1], expected, places=6)

    def test_empty_weights_return_none(self):
        self.assertIsNone(metrics.horizontal_covariance([0], [0], [0]))

    def test_degree_length_matches_runner_formula_exactly(self):
        # Shared conversion with src/bin/corpus-run.rs (R * pi / 180 per
        # degree in metres); the cross-check tolerances assume this exact
        # formula, not the rounded 111.195.
        self.assertEqual(metrics.DEG_TO_KM,
                         6_371_000.0 * math.pi / 180.0 / 1000.0)
        self.assertEqual(metrics.EARTH_RADIUS_M, 6_371_000.0)


class VerticalQuantilesTest(unittest.TestCase):
    def test_uniform_quantiles(self):
        result = metrics.vertical_quantiles([0, 10, 20, 30], [1, 1, 1, 1],
                                            quantiles=(0.5,))
        self.assertIsNotNone(result)
        # Cumulative fractions are 0.25/0.5/0.75/1.0; q=0.5 hits 10 exactly.
        self.assertAlmostEqual(result["quantiles_m"]["0.5"], 10.0)

    def test_empty_returns_none(self):
        self.assertIsNone(metrics.vertical_quantiles([], []))


class FootprintOverlapTest(unittest.TestCase):
    def test_known_overlap(self):
        result = metrics.footprint_overlap([0, 1, 1, 0], [0, 1, 0, 0])
        self.assertEqual(result["observed_active_cells"], 2)
        self.assertEqual(result["modeled_active_cells"], 1)
        self.assertEqual(result["intersection_cells"], 1)
        self.assertEqual(result["union_cells"], 2)
        self.assertAlmostEqual(result["figure_of_merit_in_space"], 0.5)

    def test_empty_union_gives_none(self):
        result = metrics.footprint_overlap([0, 0], [0, 0])
        self.assertIsNone(result["figure_of_merit_in_space"])


class EtexStationMetricsTest(unittest.TestCase):
    def test_hand_computed_fb_nmse_fac2(self):
        # obs=[10, 30], mod=[20, 20]: means 20/20, diff mean 0 -> FB=0.
        # mean sq diff = (100+100)/2=100; NMSE = 100/(20*20) = 0.25.
        # Ratios 2.0 and 0.667 are both inside [0.5, 2] -> FAC2=1.
        result = metrics.etex_station_metrics([10, 30], [20, 20])
        self.assertAlmostEqual(result["bias_pg_m3"], 0.0)
        self.assertAlmostEqual(result["fractional_bias"], 0.0)
        self.assertAlmostEqual(result["nmse"], 0.25)
        self.assertAlmostEqual(result["fac2"], 1.0)
        self.assertEqual(result["fac2_pairs"], 2)

    def test_zero_means_give_none(self):
        result = metrics.etex_station_metrics([0, 0], [0, 0])
        self.assertIsNone(result["fractional_bias"])
        self.assertIsNone(result["nmse"])
        self.assertIsNone(result["fac2"])
        self.assertIsNone(result["correlation"])

    def test_negative_values_rejected(self):
        with self.assertRaises(ValueError):
            metrics.etex_station_metrics([1], [-1])


class EtexTimingTest(unittest.TestCase):
    def test_arrival_and_peak_lags(self):
        pairs = [
            {"station": 1, "start_time": "1994-10-23 16:00", "end_time": "1994-10-23 19:00",
             "observed_pg_m3": 0.0, "gpu_pg_m3": 0.0},
            {"station": 1, "start_time": "1994-10-23 19:00", "end_time": "1994-10-23 22:00",
             "observed_pg_m3": 50.0, "gpu_pg_m3": 0.0},
            {"station": 1, "start_time": "1994-10-23 22:00", "end_time": "1994-10-24 01:00",
             "observed_pg_m3": 20.0, "gpu_pg_m3": 40.0},
        ]
        timing = metrics.etex_timing(pairs, "gpu_pg_m3", detection_threshold_pg_m3=10.0)
        self.assertEqual(timing["arrival_stations"], 1)
        # Observed arrival 19:00, modeled arrival 22:00 -> +3 h.
        self.assertAlmostEqual(timing["median_arrival_error_h"], 3.0)
        # Observed peak 19:00 (50), modeled peak 22:00 (40) -> +3 h, ratio 0.8.
        self.assertAlmostEqual(timing["median_peak_time_error_h"], 3.0)
        self.assertAlmostEqual(timing["median_peak_magnitude_ratio"], 0.8)


class SeedAggregationTest(unittest.TestCase):
    def test_known_aggregation(self):
        agg = metrics.aggregate_seed_values([1, 2, 3, 4, 5])
        self.assertEqual(agg["n_seeds"], 5)
        self.assertAlmostEqual(agg["mean"], 3.0)
        self.assertFalse(agg["sufficient_for_parity_claim"])
        full = metrics.aggregate_seed_values(list(range(10)))
        self.assertTrue(full["sufficient_for_parity_claim"])
        self.assertIsNotNone(full["ci95_mean"])

    def test_empty_raises(self):
        with self.assertRaises(ValueError):
            metrics.aggregate_seed_values([])


class UnweightedQuantilesTest(unittest.TestCase):
    def test_linear_index_convention(self):
        # Runner convention: position q*(n-1) with linear interpolation.
        result = metrics.unweighted_quantiles_linear([30, 0, 10, 20],
                                                     quantiles=(0.5,))
        self.assertAlmostEqual(result["quantiles"]["0.5"], 15.0)

    def test_single_value(self):
        result = metrics.unweighted_quantiles_linear([7.0])
        self.assertAlmostEqual(result["quantiles"]["0.1"], 7.0)

    def test_empty_raises(self):
        with self.assertRaises(ValueError):
            metrics.unweighted_quantiles_linear([])


if __name__ == "__main__":
    unittest.main()
