"""Analytical, adversarial and mutation tests for the #55 distributional metric contract.

Run from the repository root with:

    python scripts/evaluate/test_distributional.py

Expected values are derived by hand in the comments, independently of the
extraction code. Particle clouds sit on the equator so the local-tangent
longitude scale is exactly R*pi/180 (cos(0) = 1), and positions are written
from kilometre offsets with KM_PER_DEG = 6371 * pi / 180.
"""

import json
import math
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import distributional as dm
import evaluate_case
import invariants as inv
import metrics
import report as report_lib
import test_invariants as ti

CONTRACT = dm.load_contract()
KM_PER_DEG = 6371.0 * math.pi / 180.0
LON0, LAT0 = 10.0, 0.0


def km_cloud(points_km):
    """Positions (lon, lat) for east/north kilometre offsets around (LON0, LAT0)."""
    return ([LON0 + x / KM_PER_DEG for x, _ in points_km],
            [LAT0 + y / KM_PER_DEG for _, y in points_km])


def gate(verdict=inv.PASS, paired_status="ELIGIBLE", input_equivalence="INPUT_EQUIVALENT",
         realizations=None):
    return {"verdict": verdict,
            "paired_scoring": {"status": paired_status, "input_equivalence": input_equivalence,
                               "input_equivalence_report_sha256": "f" * 64, "reason": "test"},
            "realizations": realizations or []}


def realization(rid, role="candidate", verdict=inv.PASS, case_sha="c" * 64, invariants=None):
    return {"realization_id": rid, "role": role, "verdict": verdict, "seed_index": 0,
            "execution_identity": {"execution_id": rid.ljust(64, "0"), "role": role,
                                   "case_id": "CASE", "case_manifest_sha256": case_sha},
            "invariants": invariants or []}


def context(gate_doc=None, **kwargs):
    kwargs.setdefault("verify_hashes", False)
    return dm.ExtractionContext(CONTRACT, gate_doc if gate_doc is not None else gate(),
                                case_id="CASE", **kwargs)


def particles(points_km, *, heights=None, masses=None, mass_unit="kg", reference="agl",
              rid="candidate:seed_000.json", role="candidate", artifacts=None, reservoirs=None):
    lons, lats = km_cloud(points_km)
    count = len(points_km)
    return dm.particle_source(
        role, rid, lons=lons, lats=lats,
        heights=heights if heights is not None else [100.0] * count,
        masses=masses if masses is not None else [1.0] * count, mass_unit=mass_unit,
        vertical_reference=reference, artifacts=artifacts or [], species="SPECIES_A",
        reservoirs=reservoirs)


def by_id(records, metric_id, weighting=None, role=None, quantity=None):
    matches = [r for r in records if r["metric_id"] == metric_id
               and (weighting is None or r["weighting"] == weighting)
               and (role is None or r["role"] == role)
               and (quantity is None or r["quantity"] == quantity)]
    if len(matches) != 1:
        raise AssertionError(f"{len(matches)} records for {metric_id}/{weighting}/{role}")
    return matches[0]


def surface_field(values, *, nx=4, ny=1, quantity=dm.QUANTITY_DRY, measure="per_cell",
                  unit="kg", layout=dm.LAYOUT_X_FASTEST_2D, species="SPECIES_A", role="oracle",
                  rid="oracle:out", window=None, grid=None, transformation=dm.TRANSFORM_IDENTITY,
                  artifacts=None, scale=None):
    # ylat0=-0.05, dy=0.1: one row straddling the equator, so every cell has the same
    # FLEXPART zone area R^2 * rad(dx) * rad(dy).
    return dm.grid_field(
        role, rid, field_id=f"{role}/{quantity}", quantity=quantity, measure=measure, unit=unit,
        values=values, layout=layout,
        grid=grid or {"nx": nx, "ny": ny, "nz": 1, "xlon0": 0.0, "ylat0": -0.05, "dx": 0.1,
                      "dy": 0.1},
        window=window or {"kind": "accumulated_since_start", "start_epoch_s": 0,
                          "end_epoch_s": 3600},
        artifacts=artifacts or [], species=species, mass_scale_to_kg=scale,
        transformation=transformation)


def volume_field(values, *, nx=4, ny=1, nz=1, heights=(100.0,), measure="per_cell", unit="kg",
                 role="candidate", rid="candidate:grid", heights_ref="agl", species="SPECIES_A",
                 window=None, xlon0=0.0):
    return dm.grid_field(
        role, rid, field_id=f"{role}/concentration", quantity=dm.QUANTITY_CONCENTRATION,
        measure=measure, unit=unit, values=values, layout=dm.LAYOUT_XYZ,
        grid={"nx": nx, "ny": ny, "nz": nz, "xlon0": xlon0, "ylat0": -0.05, "dx": 0.1,
              "dy": 0.1, "heights_m": list(heights), "heights_ref": heights_ref},
        window=window or {"kind": "time_average", "start_epoch_s": 0, "end_epoch_s": 1800,
                          "averaging_s": 1800},
        artifacts=[], species=species)


EQUATOR_CELL_KM2 = (6_371_000.0 ** 2 * math.radians(0.1) * math.radians(0.1)) / 1e6


# ---------------------------------------------------------------------------
# Contract surface and mutation tests
# ---------------------------------------------------------------------------

class ContractTest(unittest.TestCase):
    def _mutated(self, mutate):
        document = json.loads(dm.DEFAULT_CONTRACT.read_text(encoding="utf-8"))
        mutate(document)
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "contract.json"
            path.write_text(json.dumps(document), encoding="utf-8")
            return dm.load_contract(path)

    def test_contract_is_versioned_and_machine_readable(self):
        self.assertEqual((CONTRACT["contract_id"], CONTRACT["contract_version"]),
                         (dm.CONTRACT_ID, dm.CONTRACT_VERSION))
        self.assertEqual(CONTRACT["report_schema_version"], report_lib.SCHEMA_VERSION)
        self.assertEqual({d["id"]: d["version"] for d in CONTRACT["metrics"]}, dm.IMPLEMENTED)
        self.assertEqual(CONTRACT["normative_oracle"]["pinned_commit"],
                         "c70586c2b7f5258850705325881c61f557ea9bd8")
        for definition in CONTRACT["metrics"]:
            self.assertTrue(definition["owner"])
            self.assertTrue(definition["missing_data"] and definition["degenerate"])

    def test_particle_and_grid_identifiers_never_overlap(self):
        for definition in CONTRACT["metrics"]:
            head = definition["id"].split(".")[0]
            expected = {"particle": "particle", "grid": "grid", "paired": "grid",
                        "budget": "ledger"}[head]
            self.assertEqual(definition["representation"], expected, definition["id"])

    def test_mutations_are_rejected(self):
        mutations = {
            "version": lambda d: d.__setitem__("contract_version", 2),
            "schema": lambda d: d.__setitem__("report_schema_version", "1.1.0"),
            "reason status": lambda d: d["reason_codes"].__setitem__("ZERO_VARIANCE", "BLOCKED"),
            "dropped metric": lambda d: d["metrics"].pop(),
            "metric version": lambda d: d["metrics"][0].__setitem__("version", 2),
            "embedded threshold": lambda d: d["metrics"][0].__setitem__(
                "normalization", "margin 10 percent"),
            "unknown weighting": lambda d: d["metrics"][0]["weightings"].append("area"),
            "representation": lambda d: d["metrics"][0].__setitem__("representation", "grid"),
            "missing inventory": lambda d: d["process_inventory"].pop("#7"),
            "precedence": lambda d: d["status_precedence"].reverse(),
        }
        for name, mutate in mutations.items():
            with self.subTest(name), self.assertRaises(dm.MetricContractError):
                self._mutated(mutate)

    def test_canonical_digest_ignores_line_endings(self):
        raw = dm.DEFAULT_CONTRACT.read_bytes().replace(b"\r\n", b"\n")
        with tempfile.TemporaryDirectory() as directory:
            lf, crlf = Path(directory) / "lf.json", Path(directory) / "crlf.json"
            lf.write_bytes(raw)
            crlf.write_bytes(raw.replace(b"\n", b"\r\n"))
            self.assertEqual(dm.load_contract(lf)["_sha256"], dm.load_contract(crlf)["_sha256"])


# ---------------------------------------------------------------------------
# Particle-space analytics
# ---------------------------------------------------------------------------

class ParticleAnalyticTest(unittest.TestCase):
    def _records(self, source, **ctx_kwargs):
        return dm.extract_particle_records(context(**ctx_kwargs), source)

    def test_identical_clouds_give_identical_records(self):
        points = [(1.0, 0.0), (-2.0, 1.0), (0.5, -3.0)]
        first = self._records(particles(points))
        second = self._records(particles(points))
        self.assertEqual([r["value"] for r in first], [r["value"] for r in second])

    def test_unequal_weights_two_points(self):
        # x = 0 km (w=1) and x = 10 km (w=3): centroid 7.5 km east,
        # cov_ee = (1*7.5^2 + 3*2.5^2)/4 = 18.75 km^2 (population convention).
        records = self._records(particles([(0.0, 0.0), (10.0, 0.0)], masses=[1.0, 3.0]))
        center = by_id(records, "particle.horizontal.center_of_mass", dm.WEIGHT_MASS)["value"]
        self.assertAlmostEqual(center["lon_deg"], LON0 + 7.5 / KM_PER_DEG, places=12)
        self.assertAlmostEqual(center["total_weight"], 4.0)
        cov = by_id(records, "particle.horizontal.covariance", dm.WEIGHT_MASS)["value"]
        self.assertAlmostEqual(cov["cov_east_east_km2"], 18.75, places=9)
        self.assertAlmostEqual(cov["cov_north_north_km2"], 0.0, places=12)
        # Count weighting is a different statistic: centroid 5 km, variance 25 km^2.
        count_cov = by_id(records, "particle.horizontal.covariance", dm.WEIGHT_COUNT)["value"]
        self.assertAlmostEqual(count_cov["cov_east_east_km2"], 25.0, places=9)
        self.assertNotEqual(
            by_id(records, "particle.horizontal.covariance", dm.WEIGHT_MASS)["comparability_key"],
            by_id(records, "particle.horizontal.covariance", dm.WEIGHT_COUNT)["comparability_key"])

    def test_translated_cloud_moves_center_only(self):
        base = [(1.0, 0.0), (-1.0, 0.0), (0.0, 2.0), (0.0, -2.0)]
        shifted = [(x + 25.0, y) for x, y in base]
        a, b = self._records(particles(base)), self._records(particles(shifted))
        center_a = by_id(a, "particle.horizontal.center_of_mass", dm.WEIGHT_MASS)["value"]
        center_b = by_id(b, "particle.horizontal.center_of_mass", dm.WEIGHT_MASS)["value"]
        self.assertAlmostEqual((center_b["lon_deg"] - center_a["lon_deg"]) * KM_PER_DEG, 25.0,
                               places=9)
        for metric_id in ("particle.horizontal.covariance", "particle.horizontal.principal_axes"):
            va = by_id(a, metric_id, dm.WEIGHT_MASS)["value"]
            vb = by_id(b, metric_id, dm.WEIGHT_MASS)["value"]
            for key, value in va.items():
                if isinstance(value, float):
                    self.assertAlmostEqual(value, vb[key], places=8, msg=key)

    def test_isotropic_cloud_and_expansion(self):
        # (+-d, 0), (0, +-d): cov_ee = cov_nn = d^2/2, no orientation. d=2 -> 2 km^2.
        cloud = [(2.0, 0.0), (-2.0, 0.0), (0.0, 2.0), (0.0, -2.0)]
        axes = by_id(self._records(particles(cloud)), "particle.horizontal.principal_axes",
                     dm.WEIGHT_MASS)["value"]
        self.assertAlmostEqual(axes["eigenvalue_major_km2"], 2.0, places=9)
        self.assertAlmostEqual(axes["eigenvalue_minor_km2"], 2.0, places=9)
        self.assertTrue(axes["isotropic"])
        self.assertIsNone(axes["major_axis_azimuth_deg"])
        doubled = by_id(self._records(particles([(2 * x, 2 * y) for x, y in cloud])),
                        "particle.horizontal.principal_axes", dm.WEIGHT_MASS)["value"]
        self.assertAlmostEqual(doubled["sigma_major_km"], 2 * axes["sigma_major_km"], places=9)

    def test_axis_aligned_anisotropic_cloud(self):
        # (+-3, 0), (0, +-1): cov_ee = 4.5, cov_nn = 0.5; major axis east -> azimuth 90.
        axes = by_id(self._records(particles([(3.0, 0.0), (-3.0, 0.0), (0.0, 1.0), (0.0, -1.0)])),
                     "particle.horizontal.principal_axes", dm.WEIGHT_MASS)["value"]
        self.assertAlmostEqual(axes["eigenvalue_major_km2"], 4.5, places=9)
        self.assertAlmostEqual(axes["eigenvalue_minor_km2"], 0.5, places=9)
        self.assertAlmostEqual(axes["sigma_major_km"], math.sqrt(4.5), places=9)
        self.assertAlmostEqual(axes["major_axis_azimuth_deg"], 90.0, places=9)
        north = by_id(self._records(particles([(1.0, 0.0), (-1.0, 0.0), (0.0, 3.0),
                                               (0.0, -3.0)])),
                      "particle.horizontal.principal_axes", dm.WEIGHT_MASS)["value"]
        self.assertAlmostEqual(north["major_axis_azimuth_deg"], 0.0, places=9)

    def test_diagonal_cloud(self):
        # (1, 1) and (-1, -1): cov = [[1, 1], [1, 1]], eigenvalues 2 and 0, azimuth 45.
        axes = by_id(self._records(particles([(1.0, 1.0), (-1.0, -1.0)])),
                     "particle.horizontal.principal_axes", dm.WEIGHT_MASS)["value"]
        self.assertAlmostEqual(axes["eigenvalue_major_km2"], 2.0, places=9)
        self.assertAlmostEqual(axes["eigenvalue_minor_km2"], 0.0, places=9)
        self.assertAlmostEqual(axes["major_axis_azimuth_deg"], 45.0, places=6)

    def test_single_particle(self):
        records = self._records(particles([(0.0, 0.0)]))
        cov = by_id(records, "particle.horizontal.covariance", dm.WEIGHT_MASS)["value"]
        self.assertEqual(cov["cov_east_east_km2"], 0.0)
        axes = by_id(records, "particle.horizontal.principal_axes", dm.WEIGHT_MASS)["value"]
        self.assertTrue(axes["isotropic"])
        self.assertIsNone(axes["major_axis_azimuth_deg"])
        radial = by_id(records, "particle.horizontal.radial_containment", dm.WEIGHT_MASS)
        self.assertEqual(set(radial["value"]["radius_km"].values()), {0.0})

    def test_radial_containment_radii(self):
        # Radii 1,1,2,2,3,3,4,4 km about the origin; cumulative 1/8 steps.
        # q=0.5 reaches 0.5 at the 4th entry (2 km); q=0.9 at the 8th (4 km).
        cloud = [(1, 0), (-1, 0), (0, 2), (0, -2), (3, 0), (-3, 0), (0, 4), (0, -4)]
        radial = by_id(self._records(particles([(float(x), float(y)) for x, y in cloud])),
                       "particle.horizontal.radial_containment", dm.WEIGHT_MASS)["value"]
        self.assertAlmostEqual(radial["radius_km"]["0.5"], 2.0, places=9)
        self.assertAlmostEqual(radial["radius_km"]["0.9"], 4.0, places=9)
        self.assertAlmostEqual(radial["radius_km"]["0.95"], 4.0, places=9)

    def test_vertical_quantiles_layers_and_mean(self):
        # Heights 100..400 m equal weight. q0.9: cumulative 0.75 -> 1.0 between 300 and 400,
        # fraction (0.9-0.75)/0.25 = 0.6 -> 360 m. Layers [0,150), [150,350), >= 350.
        records = self._records(particles([(0.0, 0.0)] * 4,
                                          heights=[100.0, 200.0, 300.0, 400.0]),
                                layer_heights_m=[150.0, 350.0], layer_reference="agl")
        quant = by_id(records, "particle.vertical.quantiles", dm.WEIGHT_MASS)["value"]
        self.assertEqual(quant["height_m"]["0.5"], 200.0)
        self.assertAlmostEqual(quant["height_m"]["0.9"], 360.0, places=9)
        self.assertAlmostEqual(quant["height_m"]["0.95"], 380.0, places=9)
        self.assertAlmostEqual(quant["interquartile_range_m"], 200.0, places=9)
        self.assertAlmostEqual(quant["p10_p90_range_m"], 260.0, places=9)
        self.assertEqual(by_id(records, "particle.vertical.mean_height",
                               dm.WEIGHT_MASS)["value"]["z_m"], 250.0)
        layers = by_id(records, "particle.vertical.layer_fraction", dm.WEIGHT_MASS)["value"]
        self.assertEqual(layers["layer_fraction"], [0.25, 0.5])
        self.assertEqual(layers["fraction_above_top"], 0.25)
        blh = by_id(records, "particle.vertical.fraction_above_mixing_height", dm.WEIGHT_MASS)
        self.assertEqual((blh["status"], blh["reason_code"]),
                         (dm.UNSUPPORTED, "MIXING_HEIGHT_UNAVAILABLE"))

    def test_zero_mass_is_degenerate_not_fabricated(self):
        records = self._records(particles([(0.0, 0.0), (5.0, 0.0)], masses=[0.0, 0.0]))
        center = by_id(records, "particle.horizontal.center_of_mass", dm.WEIGHT_MASS)
        self.assertEqual((center["status"], center["reason_code"], center["value"]),
                         (dm.DEGENERATE, "ZERO_TOTAL_WEIGHT", None))
        count_center = by_id(records, "particle.horizontal.center_of_mass", dm.WEIGHT_COUNT)
        self.assertEqual(count_center["status"], dm.COMPUTED)

    def test_records_are_strict_json(self):
        records = self._records(particles([(1.0, 2.0), (3.0, -1.0)]))
        json.dumps(records, allow_nan=False)
        for record in records:
            for key in ("metric_id", "definition_version", "status", "units", "representation",
                        "weighting", "quantity", "realization", "case", "time_window",
                        "source_artifacts", "comparability_key",
                        "eligible_for_paired_scoring"):
                self.assertIn(key, record)


# ---------------------------------------------------------------------------
# Particle-space adversarial cases
# ---------------------------------------------------------------------------

class ParticleNegativeTest(unittest.TestCase):
    def _status(self, source, metric_id, weighting=dm.WEIGHT_MASS, **ctx_kwargs):
        record = by_id(dm.extract_particle_records(context(**ctx_kwargs), source), metric_id,
                       weighting)
        if record["status"] != dm.COMPUTED:
            self.assertIsNone(record["value"])
        return record["status"], record["reason_code"]

    def test_missing_particle_dump(self):
        source = dm.particle_source("oracle", "oracle:out", lons=None, lats=None, heights=None,
                                    masses=None, mass_unit=None, vertical_reference="agl",
                                    artifacts=[])
        self.assertEqual(self._status(source, "particle.horizontal.center_of_mass",
                                      dm.WEIGHT_COUNT),
                         (dm.MISSING_EVIDENCE, "PARTICLE_DUMP_UNAVAILABLE"))

    def test_oracle_without_masses_is_count_only(self):
        source = particles([(0.0, 0.0), (1.0, 0.0)], masses=None)
        source["masses"] = None
        self.assertEqual(self._status(source, "particle.horizontal.covariance"),
                         (dm.MISSING_EVIDENCE, "PARTICLE_MASS_UNAVAILABLE"))
        self.assertEqual(self._status(source, "particle.horizontal.covariance", dm.WEIGHT_COUNT),
                         (dm.COMPUTED, None))

    def test_incorrect_weight_units(self):
        source = particles([(0.0, 0.0), (1.0, 0.0)], mass_unit="g")
        self.assertEqual(self._status(source, "particle.horizontal.center_of_mass"),
                         (dm.INTEGRITY_ERROR, "WEIGHT_UNIT_INVALID"))

    def test_negative_and_nonfinite_weights(self):
        self.assertEqual(self._status(particles([(0.0, 0.0), (1.0, 0.0)], masses=[1.0, -1.0]),
                                      "particle.horizontal.covariance"),
                         (dm.INTEGRITY_ERROR, "NEGATIVE_WEIGHT"))
        self.assertEqual(self._status(particles([(0.0, 0.0), (1.0, 0.0)],
                                                masses=[1.0, float("nan")]),
                                      "particle.vertical.quantiles"),
                         (dm.INTEGRITY_ERROR, "NONFINITE_VALUE"))

    def test_invalid_coordinates_and_wrap(self):
        source = particles([(0.0, 0.0)])
        source["lats"] = [95.0]
        self.assertEqual(self._status(source, "particle.horizontal.center_of_mass"),
                         (dm.INTEGRITY_ERROR, "INVALID_COORDINATE"))
        wrap = particles([(0.0, 0.0), (0.0, 0.0)])
        wrap["lons"] = [179.9, -179.9]
        self.assertEqual(self._status(wrap, "particle.horizontal.center_of_mass"),
                         (dm.UNSUPPORTED, "LONGITUDE_WRAP_UNSUPPORTED"))
        negative_agl = particles([(0.0, 0.0)], heights=[-5.0])
        self.assertEqual(self._status(negative_agl, "particle.vertical.mean_height"),
                         (dm.INTEGRITY_ERROR, "INVALID_COORDINATE"))

    def test_incompatible_agl_asl_layers(self):
        source = particles([(0.0, 0.0)], heights=[10.0], reference="asl")
        self.assertEqual(self._status(source, "particle.vertical.layer_fraction",
                                      layer_heights_m=[100.0], layer_reference="agl"),
                         (dm.INCOMPATIBLE, "VERTICAL_REFERENCE_MISMATCH"))
        record = by_id(dm.extract_particle_records(context(), source),
                       "particle.vertical.mean_height", dm.WEIGHT_MASS)
        self.assertEqual(record["vertical_reference"], "asl")
        self.assertIn("|asl|", record["comparability_key"])

    def test_mutated_artifact_hash(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "seed_000.json"
            path.write_text("original", encoding="utf-8")
            artifact = {"label": "seed_000.json", "path": str(path),
                        "sha256": report_lib.sha256_file(path)}
            path.write_text("mutated", encoding="utf-8")
            records = dm.extract_particle_records(
                context(verify_hashes=True), particles([(0.0, 0.0)], artifacts=[artifact]))
        self.assertEqual({(r["status"], r["reason_code"]) for r in records},
                         {(dm.INTEGRITY_ERROR, "ARTIFACT_HASH_MISMATCH")})

    def test_negative_eigenvalue_beyond_roundoff_is_rejected(self):
        with self.assertRaises(ValueError):
            metrics.principal_axes([[1.0, 2.0], [2.0, 1.0]])
        clamped = metrics.principal_axes([[1.0, 1.0], [1.0, 1.0 - 1e-15]])
        self.assertTrue(clamped["roundoff_clamped"])
        self.assertEqual(clamped["eigenvalue_minor_km2"], 0.0)

    def test_nonfinite_value_is_never_emitted(self):
        record = dm.make_record(context(), "particle.horizontal.center_of_mass", role="candidate",
                                value={"lon_deg": float("inf")}, weighting=dm.WEIGHT_MASS,
                                quantity=dm.QUANTITY_PARTICLES)
        self.assertEqual((record["status"], record["value"]), (dm.INTEGRITY_ERROR, None))


# ---------------------------------------------------------------------------
# Gridded-field analytics
# ---------------------------------------------------------------------------

class GridAnalyticTest(unittest.TestCase):
    def test_footprint_hotspot_and_center(self):
        # Equal-area cells with mass 6, 3, 1 (per_cell): 50% needs cell 0 (0.6),
        # 90% needs cells 0+1 (0.9). Center lon = (6*.05 + 3*.15 + 1*.25)/10 = 0.1.
        records = dm.extract_grid_records(context(), surface_field([6.0, 3.0, 1.0], nx=3))
        footprint = by_id(records, "grid.horizontal.mass_footprint_area")["value"]
        self.assertEqual(footprint["cell_count"], {"0.5": 1, "0.9": 2})
        self.assertAlmostEqual(footprint["area_km2"]["0.5"], EQUATOR_CELL_KM2, places=6)
        self.assertAlmostEqual(footprint["area_km2"]["0.9"], 2 * EQUATOR_CELL_KM2, places=6)
        hotspot = by_id(records, "grid.horizontal.hotspot")["value"]
        self.assertEqual((hotspot["ix"], hotspot["iy"], hotspot["tied_cells"]), (0, 0, 1))
        self.assertAlmostEqual(hotspot["mass_fraction"], 0.6)
        self.assertAlmostEqual(hotspot["lon_deg"], 0.05)
        center = by_id(records, "grid.horizontal.center_of_mass")["value"]
        self.assertAlmostEqual(center["lon_deg"], 0.1, places=12)

    def test_deposition_layout_and_known_hotspot(self):
        # x_fastest_2d index ix + iy*nx: value 5 at index 3 is cell (ix=1, iy=1).
        field = surface_field([0.0, 0.0, 0.0, 5.0], nx=2, ny=2, measure="per_area",
                              grid={"nx": 2, "ny": 2, "nz": 1, "xlon0": 0.0, "ylat0": 0.0,
                                    "dx": 0.1, "dy": 0.1})
        records = dm.extract_grid_records(context(), field)
        hotspot = by_id(records, "grid.horizontal.hotspot")["value"]
        self.assertEqual((hotspot["ix"], hotspot["iy"], hotspot["mass_fraction"]), (1, 1, 1.0))
        self.assertAlmostEqual(hotspot["lat_deg"], 0.15)
        total = by_id(records, "grid.field.integrated_total")
        self.assertEqual((total["status"], total["reason_code"]),
                         (dm.UNSUPPORTED, "FIELD_MASS_SCALE_UNDECLARED"))

    def test_declared_scale_gives_kilogram_total(self):
        records = dm.extract_grid_records(context(), surface_field([1.0, 2.0, 0.0, 1.0],
                                                                   scale=1.0))
        self.assertEqual(by_id(records, "grid.field.integrated_total")["value"], {"total_kg": 4.0})

    def test_nonuniform_layers_use_volume(self):
        # One column, uniform concentration, layers 0-100 m and 100-300 m: mass splits
        # 1/3 : 2/3 (summed concentration would wrongly give 1/2 : 1/2); mean height
        # = 50/3 + 200*2/3 = 150 m from layer midpoints 50 m and 200 m.
        field = volume_field([2.0, 2.0], nx=1, nz=2, heights=(100.0, 300.0), measure="per_volume",
                             unit="native", role="oracle", rid="oracle:out")
        records = dm.extract_grid_records(context(), field)
        layers = by_id(records, "grid.vertical.layer_fraction")["value"]["layer_fraction"]
        self.assertAlmostEqual(layers[0], 1.0 / 3.0, places=12)
        self.assertAlmostEqual(layers[1], 2.0 / 3.0, places=12)
        self.assertAlmostEqual(by_id(records, "grid.vertical.mean_height")["value"]["z_m"], 150.0,
                               places=9)

    def test_empty_field_is_degenerate(self):
        records = dm.extract_grid_records(context(), surface_field([0.0] * 4, scale=1.0))
        center = by_id(records, "grid.horizontal.center_of_mass")
        self.assertEqual((center["status"], center["value"]), (dm.DEGENERATE, None))
        self.assertEqual(by_id(records, "grid.field.integrated_total")["value"],
                         {"total_kg": 0.0})

    def test_single_cell(self):
        records = dm.extract_grid_records(context(), surface_field([0.0, 7.0, 0.0, 0.0]))
        axes = by_id(records, "grid.horizontal.principal_axes")["value"]
        self.assertTrue(axes["isotropic"])
        self.assertIsNone(axes["major_axis_azimuth_deg"])
        self.assertAlmostEqual(axes["eigenvalue_major_km2"], 0.0, places=12)


# ---------------------------------------------------------------------------
# Paired gridded comparisons
# ---------------------------------------------------------------------------

def paired_gate(**kwargs):
    reals = [realization("candidate:grid"), realization("oracle:out", role="oracle")]
    return gate(realizations=reals, **kwargs)


def pair(candidate_values, oracle_values, gate_doc=None, candidate_kwargs=None,
         oracle_kwargs=None):
    candidate = volume_field(candidate_values, **(candidate_kwargs or {}))
    oracle = volume_field(oracle_values, role="oracle", rid="oracle:out", **(oracle_kwargs or {}))
    records = dm.extract_paired_grid_records(context(gate_doc or paired_gate()), candidate,
                                             oracle)
    return {r["metric_id"]: r for r in records}


class PairedAnalyticTest(unittest.TestCase):
    def test_identical_fields(self):
        records = pair([1.0, 2.0, 3.0, 4.0], [1.0, 2.0, 3.0, 4.0])
        stats = records["paired.grid.field_statistics"]["value"]
        self.assertEqual((stats["mean_bias"], stats["rmse"]), (0.0, 0.0))
        self.assertAlmostEqual(stats["pearson_correlation"], 1.0)
        self.assertAlmostEqual(records["paired.grid.shape_statistics"]["value"][
            "normalized_mass_overlap"], 1.0)
        self.assertEqual(records["paired.grid.support_overlap"]["value"][
            "figure_of_merit_in_space"], 1.0)
        self.assertEqual(records["paired.grid.center_separation"]["value"]["separation_km"], 0.0)
        self.assertTrue(all(r["eligible_for_paired_scoring"] for r in records.values()))
        self.assertEqual(records["paired.grid.field_statistics"]["input_equivalence"]["verdict"],
                         "INPUT_EQUIVALENT")

    def test_additive_bias(self):
        # o = [1,2,3,4], c = o + 1: bias 1, RMSE 1, mean_o 2.5, mean_c 3.5,
        # relative bias 0.4, NRMSE 1/3.5, FB 2*1/6, NMSE 1/(2.5*3.5), r = 1.
        stats = pair([2.0, 3.0, 4.0, 5.0], [1.0, 2.0, 3.0, 4.0])[
            "paired.grid.field_statistics"]["value"]
        self.assertAlmostEqual(stats["mean_bias"], 1.0)
        self.assertAlmostEqual(stats["rmse"], 1.0)
        self.assertAlmostEqual(stats["relative_mean_bias"], 0.4)
        self.assertAlmostEqual(stats["normalized_rmse"], 1.0 / 3.5)
        self.assertAlmostEqual(stats["fractional_bias"], 1.0 / 3.0)
        self.assertAlmostEqual(stats["nmse"], 1.0 / (2.5 * 3.5))
        self.assertAlmostEqual(stats["pearson_correlation"], 1.0)

    def test_multiplicative_bias_keeps_shape_but_not_magnitude(self):
        records = pair([2.0, 4.0, 6.0, 8.0], [1.0, 2.0, 3.0, 4.0])
        stats = records["paired.grid.field_statistics"]["value"]
        self.assertAlmostEqual(stats["pearson_correlation"], 1.0)
        self.assertAlmostEqual(stats["relative_mean_bias"], 1.0)
        self.assertAlmostEqual(stats["mean_bias"], 2.5)
        shape = records["paired.grid.shape_statistics"]["value"]
        self.assertAlmostEqual(shape["normalized_mass_overlap"], 1.0)
        self.assertAlmostEqual(shape["normalized_rmse"], 0.0, places=12)

    def test_constant_field_correlation_is_undefined(self):
        stats = pair([1.0, 2.0, 3.0, 4.0], [2.0, 2.0, 2.0, 2.0])[
            "paired.grid.field_statistics"]["value"]
        self.assertIsNone(stats["pearson_correlation"])
        self.assertIn("correlation", stats["undefined"])

    def test_zero_reference_denominators(self):
        records = pair([1.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 0.0])
        stats = records["paired.grid.field_statistics"]["value"]
        self.assertIsNone(stats["relative_mean_bias"])
        self.assertIsNone(stats["nmse"])
        self.assertIn("relative_mean_bias", stats["undefined"])
        shape = records["paired.grid.shape_statistics"]
        self.assertEqual((shape["status"], shape["reason_code"]),
                         (dm.DEGENERATE, "ZERO_TOTAL_WEIGHT"))

    def test_non_and_partially_overlapping_footprints(self):
        disjoint = pair([0.0, 0.0, 0.0, 1.0], [1.0, 0.0, 0.0, 0.0])
        self.assertEqual(disjoint["paired.grid.support_overlap"]["value"][
            "figure_of_merit_in_space"], 0.0)
        self.assertEqual(disjoint["paired.grid.shape_statistics"]["value"][
            "normalized_mass_overlap"], 0.0)
        # o on cells {0,1}, c on {1,2}: support IoU 1/3; normalized overlap 0.5;
        # 90% footprints {0,1} and {1,2} -> area IoU 1/3; 50% footprints {0} (tie -> lowest
        # index) and {1} -> 0. Center offset 0.1 deg east at the equator.
        partial = pair([0.0, 1.0, 1.0, 0.0], [1.0, 1.0, 0.0, 0.0])
        self.assertAlmostEqual(partial["paired.grid.support_overlap"]["value"][
            "figure_of_merit_in_space"], 1.0 / 3.0)
        self.assertAlmostEqual(partial["paired.grid.shape_statistics"]["value"][
            "normalized_mass_overlap"], 0.5)
        ratios = partial["paired.grid.mass_footprint_overlap"]["value"]["area_overlap_ratio"]
        self.assertAlmostEqual(ratios["0.9"], 1.0 / 3.0)
        self.assertEqual(ratios["0.5"], 0.0)
        self.assertAlmostEqual(partial["paired.grid.center_separation"]["value"]["separation_km"],
                               0.1 * KM_PER_DEG, places=9)

    def test_unit_scale_difference_blocks_absolute_only(self):
        records = pair([1.0, 2.0, 3.0, 4.0], [1.0, 2.0, 3.0, 4.0],
                       oracle_kwargs={"unit": "g"})
        self.assertEqual(records["paired.grid.field_statistics"]["reason_code"], "UNIT_MISMATCH")
        self.assertEqual(records["paired.grid.shape_statistics"]["status"], dm.COMPUTED)


class PairedNegativeTest(unittest.TestCase):
    def _code(self, records, metric_id="paired.grid.shape_statistics"):
        record = records[metric_id]
        self.assertIsNone(record["value"])
        self.assertFalse(record["eligible_for_paired_scoring"])
        return record["status"], record["reason_code"]

    def test_incompatible_domains(self):
        cases = {
            "grid shape": ({"nx": 2, "ny": 2}, (dm.INCOMPATIBLE, "GRID_MISMATCH")),
            "extent": ({"xlon0": 0.5}, (dm.INCOMPATIBLE, "GRID_MISMATCH")),
            "window": ({"window": {"kind": "time_average", "start_epoch_s": 1800,
                                   "end_epoch_s": 3600, "averaging_s": 1800}},
                       (dm.INCOMPATIBLE, "WINDOW_MISMATCH")),
            "species": ({"species": "SPECIES_B"}, (dm.INCOMPATIBLE, "SPECIES_MISMATCH")),
            "undeclared species": ({"species": None}, (dm.INCOMPATIBLE, "SPECIES_UNDECLARED")),
            "asl": ({"heights_ref": "asl"}, (dm.INCOMPATIBLE, "VERTICAL_REFERENCE_MISMATCH")),
        }
        for name, (overrides, expected) in cases.items():
            with self.subTest(name):
                self.assertEqual(self._code(pair([1.0] * 4, [1.0] * 4,
                                                 oracle_kwargs=overrides)), expected)

    def test_wrong_quantity(self):
        candidate = surface_field([1.0] * 4, quantity=dm.QUANTITY_DRY, role="candidate",
                                  rid="candidate:grid")
        oracle = surface_field([1.0] * 4, quantity=dm.QUANTITY_WET)
        records = {r["metric_id"]: r for r in dm.extract_paired_grid_records(
            context(paired_gate()), candidate, oracle)}
        self.assertEqual(self._code(records), (dm.INCOMPATIBLE, "QUANTITY_MISMATCH"))

    def test_regridding_is_never_implicit(self):
        for transformation, code in (("bilinear", "NONCONSERVATIVE_TRANSFORMATION"),
                                     (dm.TRANSFORM_CONSERVATIVE_REGRID,
                                      "REGRIDDING_UNAVAILABLE")):
            with self.subTest(transformation):
                candidate = surface_field([1.0] * 4, role="candidate", rid="candidate:grid",
                                          transformation=transformation)
                records = {r["metric_id"]: r for r in dm.extract_paired_grid_records(
                    context(paired_gate()), candidate, surface_field([1.0] * 4))}
                self.assertEqual(self._code(records), (dm.INCOMPATIBLE, code))

    def test_incomplete_fields(self):
        self.assertEqual(self._code(pair([1.0] * 3, [1.0] * 4)),
                         (dm.INTEGRITY_ERROR, "INCOMPLETE_FIELD"))
        self.assertEqual(self._code(pair([1.0, None, 1.0, 1.0], [1.0] * 4)),
                         (dm.INTEGRITY_ERROR, "INCOMPLETE_FIELD"))
        missing = dm.extract_grid_records(context(), volume_field(None))
        self.assertEqual(by_id(missing, "grid.horizontal.center_of_mass")["reason_code"],
                         "GRID_FIELD_UNAVAILABLE")

    def test_mixed_execution_provenance(self):
        reals = [realization("candidate:grid"),
                 realization("oracle:out", role="oracle", case_sha="d" * 64)]
        records = pair([1.0] * 4, [1.0] * 4, gate_doc=gate(realizations=reals))
        self.assertEqual(self._code(records),
                         (dm.INTEGRITY_ERROR, "MIXED_EXECUTION_PROVENANCE"))

    def test_missing_input_equivalence_and_binding(self):
        self.assertEqual(self._code(pair([1.0] * 4, [1.0] * 4, gate_doc=paired_gate(
            paired_status="BLOCKED", input_equivalence="INPUT_EQUIVALENCE_NOT_DEMONSTRATED"))),
            (dm.BLOCKED, "INPUT_EQUIVALENCE_NOT_DEMONSTRATED"))
        self.assertEqual(self._code(pair([1.0] * 4, [1.0] * 4, gate_doc=paired_gate(
            paired_status="BLOCKED"))), (dm.BLOCKED, "PROVENANCE_NOT_BOUND"))

    def test_failed_invariant_beats_excellent_metrics(self):
        # Identical fields would give perfect paired metrics; a #54 FAIL blocks all of them.
        fail_gate = paired_gate(verdict=inv.FAIL)
        records = pair([1.0, 2.0, 3.0, 4.0], [1.0, 2.0, 3.0, 4.0], gate_doc=fail_gate)
        for metric_id in dm.PAIRED_GRID_IDS:
            self.assertEqual(self._code(records, metric_id),
                             (dm.BLOCKED, "INVARIANT_GATE_NOT_PASS"))
        unary = dm.extract_grid_records(context(fail_gate), volume_field([1.0, 2.0, 3.0, 4.0]))
        self.assertTrue(all(not r["eligible_for_paired_scoring"] for r in unary))


# ---------------------------------------------------------------------------
# Ledger records and downstream handoff
# ---------------------------------------------------------------------------

class LedgerAndHandoffTest(unittest.TestCase):
    def test_budget_reads_invariant_record(self):
        measured = metrics.mass_budget(1.0, 0.7, dry_deposited_kg=0.3)
        closure = {"invariant_id": "INV-MASS-CLOSURE", "verdict": inv.PASS, "measured": measured}
        ctx = context(gate(realizations=[realization("candidate:seed_000.json",
                                                     invariants=[closure])]))
        source = particles([(0.0, 0.0)], reservoirs={"dry_deposited_kg": 0.3})
        records = dm.budget_records(ctx, source)
        budget = by_id(records, "budget.mass_closure")["value"]
        self.assertEqual((budget["dry_deposited_kg"], budget["invariant_verdict"]),
                         (0.3, inv.PASS))
        self.assertEqual(by_id(records, "budget.deposited_mass_total",
                               quantity=dm.QUANTITY_DRY)["value"], {"total_kg": 0.3})
        wet = by_id(records, "budget.deposited_mass_total", quantity=dm.QUANTITY_WET)
        self.assertEqual((wet["status"], wet["reason_code"]),
                         (dm.MISSING_EVIDENCE, "DEPOSITION_RESERVOIR_UNAVAILABLE"))

    def test_disabled_process_has_no_reservoir_record(self):
        records = dm.budget_records(context(), particles([(0.0, 0.0)], reservoirs={}),
                                    quantities=[dm.QUANTITY_DRY])
        self.assertEqual([r["quantity"] for r in records
                          if r["metric_id"] == "budget.deposited_mass_total"], [dm.QUANTITY_DRY])

    def test_oracle_ledger_is_missing_not_reconstructed(self):
        blocked = {"invariant_id": "INV-MASS-CLOSURE", "verdict": inv.BLOCKED,
                   "error_id": "MASS_LEDGER_UNAVAILABLE", "detail": "no ledger"}
        ctx = context(gate(realizations=[realization("oracle:out", role="oracle",
                                                     invariants=[blocked])]))
        source = dm.particle_source("oracle", "oracle:out", lons=None, lats=None, heights=None,
                                    masses=None, mass_unit=None, vertical_reference=None,
                                    artifacts=[])
        records = dm.budget_records(ctx, source)
        self.assertEqual({r["reason_code"] for r in records},
                         {"MASS_LEDGER_UNAVAILABLE", "DEPOSITION_RESERVOIR_UNAVAILABLE"})

    def _report(self, verdict=inv.PASS, contract=CONTRACT):
        ctx = dm.ExtractionContext(contract, gate(verdict, realizations=[
            dict(realization("candidate:seed_000.json", verdict=verdict),
                 invariants=[{"invariant_id": "INV-MASS-CLOSURE", "verdict": verdict}])]),
            case_id="CASE", verify_hashes=False)
        records = dm.extract_particle_records(ctx, particles([(0.0, 0.0), (1.0, 0.0)]))
        return {"distributional_metrics": dm.build_block(ctx, records, extraction_scope="test"),
                "deterministic_invariants": ctx.gate}

    def test_handoff_keeps_failed_invariants_visible(self):
        handoff = dm.collect_for_ensemble([("a.json", self._report()),
                                           ("b.json", self._report(inv.FAIL))])
        self.assertFalse(handoff["eligible"])
        self.assertEqual(handoff["deterministic_invariants"]["verdict"], inv.FAIL)
        self.assertEqual(handoff["ineligible_reports"], ["b.json"])
        self.assertTrue(all(key.startswith("candidate::") for key in handoff["groups"]))
        ok = dm.collect_for_ensemble([("a.json", self._report())])
        self.assertTrue(ok["eligible"])

    def test_handoff_rejects_mixed_contract_versions(self):
        other = self._report()
        other["distributional_metrics"]["contract_version"] = 2
        with self.assertRaises(dm.MetricContractError):
            dm.collect_for_ensemble([("a.json", self._report()), ("b.json", other)])
        digest = self._report()
        digest["distributional_metrics"]["contract_sha256"] = "0" * 64
        with self.assertRaises(dm.MetricContractError):
            dm.collect_for_ensemble([("a.json", digest)])

    def test_unextracted_report_is_ineligible(self):
        handoff = dm.collect_for_ensemble([("a.json", self._report()),
                                           ("b.json", {"distributional_metrics": None,
                                                       "deterministic_invariants": None})])
        self.assertFalse(handoff["eligible"])
        self.assertEqual(handoff["deterministic_invariants"]["verdict"], inv.BLOCKED)


# ---------------------------------------------------------------------------
# Evaluator integration and command line
# ---------------------------------------------------------------------------

class EvaluatorIntegrationTest(unittest.TestCase):
    def _corpus(self, root, masses=None, oracle=False, **overrides):
        case_path = ti.write_case(root / "ADV-ANA-001.json", ti.adv_case())
        seed = ti.write_seed(root / "seed_000.json", "ADV-ANA-001", 0, ti.adv_positions(4),
                             masses or [1.0] * 4)
        if oracle:
            overrides["fortran_output"] = str(ti.write_flexpart_output(root / "oracle"))
        args = ti.args_for(case_path, [seed], metric_contract=str(dm.DEFAULT_CONTRACT),
                           **overrides)
        return evaluate_case.run_corpus_seeds(args, ti.ORACLE_MANIFEST)

    def test_corpus_report_carries_contract_records(self):
        with tempfile.TemporaryDirectory() as directory:
            built = self._corpus(Path(directory), oracle=True)
        block = built["distributional_metrics"]
        self.assertEqual(built["schema_version"], "1.2.0")
        self.assertEqual(block["contract_sha256"], CONTRACT["_sha256"])
        json.dumps(built, allow_nan=False)
        records = block["records"]
        # All four particles share one analytic end position (10E + 36 km, 50N).
        center = by_id(records, "particle.horizontal.center_of_mass", dm.WEIGHT_MASS,
                       role="candidate")
        self.assertAlmostEqual(center["value"]["lon_deg"], ti.ADV_LON0 + ti.ADV_EAST_M /
                               ti.M_PER_DEG_LON, places=9)
        self.assertEqual(center["realization"]["stochastic_identity"]["namespace"],
                         dm.STOCHASTIC_CANDIDATE)
        self.assertEqual(center["case"]["case_id"], "ADV-ANA-001")
        # Oracle uniform concentration 2.0 in layers 0-100 and 100-250 m: volume fractions
        # 100/250 and 150/250.
        # Two output times -> one record per grid_conc file, each with its own field id.
        layers = [r for r in records if r["metric_id"] == "grid.vertical.layer_fraction"
                  and r["role"] == "oracle"]
        self.assertEqual(len({r["field_ids"][0] for r in layers}), 2)
        for record in layers:
            self.assertAlmostEqual(record["value"]["layer_fraction"][0], 0.4, places=6)
            self.assertEqual(record["time_window"]["kind"], "time_average")
        self.assertEqual(by_id(records, "particle.horizontal.center_of_mass", dm.WEIGHT_COUNT,
                               role="oracle")["reason_code"], "PARTICLE_DUMP_UNAVAILABLE")
        paired = [r for r in records if r["classification"] == "paired"]
        self.assertTrue(paired)
        self.assertEqual({r["reason_code"] for r in paired}, {"GRID_FIELD_UNAVAILABLE"})
        self.assertFalse(any(r["eligible_for_paired_scoring"] for r in records))
        # ADV-ANA-001 disables deposition: no deposition quantities are in scope.
        grid_quantities = {r["quantity"] for r in records if r["representation"] == "grid"}
        self.assertEqual(grid_quantities, {dm.QUANTITY_CONCENTRATION})

    def test_invariant_fail_is_not_hidden_by_metrics(self):
        with tempfile.TemporaryDirectory() as directory:
            built = self._corpus(Path(directory), masses=[0.5] * 4)
        self.assertEqual(built["overall_status"], "FAIL")
        block = built["distributional_metrics"]
        self.assertEqual(block["deterministic_invariants_verdict"], inv.FAIL)
        center = by_id(block["records"], "particle.horizontal.center_of_mass", dm.WEIGHT_MASS,
                       role="candidate")
        self.assertEqual(center["status"], dm.COMPUTED)
        self.assertEqual(center["realization"]["invariant_verdict"], inv.FAIL)
        self.assertFalse(center["eligible_for_paired_scoring"])
        budget = by_id(block["records"], "budget.mass_closure", role="candidate")
        self.assertEqual(budget["value"]["invariant_verdict"], inv.FAIL)

    def test_cli_and_contract_mutation(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            case_path = ti.write_case(root / "ADV-ANA-001.json", ti.adv_case())
            seed = ti.write_seed(root / "seed_000.json", "ADV-ANA-001", 0, ti.adv_positions(4),
                                 [1.0] * 4)
            base = [sys.executable, str(HERE / "evaluate_case.py"), "--case", "corpus-seeds",
                    "--case-def", str(case_path), "--candidate-seeds", str(seed)]
            ok = subprocess.run(base + ["--output", str(root / "ok.json")],
                                capture_output=True, text=True)
            report = json.loads((root / "ok.json").read_text(encoding="utf-8"))
            self.assertEqual(ok.returncode, 0, ok.stderr)
            self.assertGreater(report["distributional_metrics"]["record_count"], 0)
            self.assertIn("Distributional metrics", ok.stdout)
            mutated = json.loads(dm.DEFAULT_CONTRACT.read_text(encoding="utf-8"))
            mutated["contract_version"] = 2
            bad = root / "bad-contract.json"
            bad.write_text(json.dumps(mutated), encoding="utf-8")
            failed = subprocess.run(base + ["--output", str(root / "bad.json"),
                                            "--metric-contract", str(bad)],
                                    capture_output=True, text=True)
            bad_report = json.loads((root / "bad.json").read_text(encoding="utf-8"))
            self.assertEqual(failed.returncode, 2)
            self.assertEqual(bad_report["overall_status"], "INTEGRITY_ERROR")
            self.assertIsNone(bad_report["distributional_metrics"])

            aggregate = subprocess.run(
                [sys.executable, str(HERE / "evaluate_case.py"), "--case", "aggregate-seeds",
                 "--seed-reports", str(root / "ok.json"), "--output", str(root / "agg.json")],
                capture_output=True, text=True)
            agg = json.loads((root / "agg.json").read_text(encoding="utf-8"))
            self.assertEqual(aggregate.returncode, 0, aggregate.stderr)
            self.assertEqual(agg["distributional_metrics"]["record_count"],
                             report["distributional_metrics"]["record_count"])
            self.assertTrue(all(r["source_report"] for r in agg["distributional_metrics"][
                "records"]))
            report["distributional_metrics"]["contract_sha256"] = "0" * 64
            (root / "stale.json").write_text(json.dumps(report), encoding="utf-8")
            stale = subprocess.run(
                [sys.executable, str(HERE / "evaluate_case.py"), "--case", "aggregate-seeds",
                 "--seed-reports", str(root / "stale.json"), "--output",
                 str(root / "agg2.json")], capture_output=True, text=True)
            self.assertEqual(stale.returncode, 2)


if __name__ == "__main__":
    unittest.main()
