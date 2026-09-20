#!/usr/bin/env python3
"""Audit corpus input equality between case JSON and oracle fixtures.

Checks, for every implemented synthetic case with a case JSON:

- Fortran OUTGRID mirrors the case ``domain`` (origin, size, spacing).
- Fortran RELEASES mirrors the case ``release`` (position, particle count)
  with MASS converted kg -> g (``MASS_g = mass_kg * 1000``).
- Fortran COMMAND switches mirror ``oracle_command_overrides``.
- The expected SPECIES file (inert 024, depositing 040 for DRY/WET) exists.
- The derived fixtures regenerate byte-identically (via
  generate_fortran_fixtures verification).

With ``--candidate-dir``, additionally checks every present candidate seed
file against its case JSON (particle count, initial mass, Philox key
derivation, non-empty adapter). With ``--oracle-dir --require-oracle``,
requires a decoded ``oracle_summary.json`` per implemented case (except
REPEAT-009, which has no oracle seed control) whose release mass matches
RELEASES.

Exit status is non-zero on any mismatch. Comparison and manifest steps run
this audit first so outputs are never compared from unequal inputs.

Usage:
    python3 scripts/corpus/audit_corpus_inputs.py [--fixtures fixtures/corpus]
    python3 scripts/corpus/audit_corpus_inputs.py --candidate-dir target/corpus/candidate
"""

import argparse
import importlib.util
import json
import math
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "scripts" / "corpus"))


def load_generator():
    spec = importlib.util.spec_from_file_location(
        "generate_fortran_fixtures",
        REPO / "scripts" / "corpus" / "generate_fortran_fixtures.py",
    )
    module = importlib.util.module_from_spec(spec)
    assert spec.loader is not None
    spec.loader.exec_module(module)
    return module


GEN = load_generator()

FAILURES: list = []

PHILOX_DERIVATION_WRAPPING_ADD_KEY0_V1 = "wrapping_add_key0_v1"
PHILOX_DERIVATION_REUSE_BASE_IDENTITY_V1 = "reuse_base_identity_v1"
SUPPORTED_PHILOX_DERIVATIONS = {
    PHILOX_DERIVATION_WRAPPING_ADD_KEY0_V1,
    PHILOX_DERIVATION_REUSE_BASE_IDENTITY_V1,
}


def check(name: str, ok: bool, detail: str = "") -> None:
    print(("PASS " if ok else "FAIL ") + name + (f" ({detail})" if detail and not ok else ""))
    if not ok:
        FAILURES.append(name + (f": {detail}" if detail else ""))


def audit_fixture_case(case_id: str, case: dict, fort_dir: Path) -> None:
    outdir = fort_dir / case_id
    if not outdir.is_dir():
        check(f"{case_id} fortran fixture present", False, f"missing {outdir}")
        return
    try:
        physics = GEN.mandatory_physics_switches(case_id, case)
        GEN._validate_species_physics_contract(case_id, case, physics)
        GEN._validate_deposition_contract(case_id, case, physics)
        specnum = GEN.species_number_for_case(case_id, case)
        GEN.verify_case(case_id, case, outdir, specnum)
        check(f"{case_id} fixture equals case JSON", True)
    except SystemExit as exc:
        check(f"{case_id} fixture equals case JSON", False, str(exc))
    species = outdir / "SPECIES" / f"SPECIES_{specnum:03d}"
    check(f"{case_id} SPECIES_{specnum:03d} present", species.is_file())
    if species.is_file():
        import re as _re

        code = _re.sub(r"!.*", "", species.read_text(encoding="utf-8"))
        check(
            f"{case_id} SPECIES v11.1-readable (no PNDIA)",
            _re.search(r"(?im)^\s*PNDIA\s*=", code) is None,
        )
        if case_id == "DRY-007":
            pdryvel = GEN.namelist_value(
                species.read_text(encoding="utf-8"), "PDRYVEL"
            ).strip()
            check(
                f"{case_id} SPECIES PDRYVEL=2.0 (0.02 m/s, candidate-equivalent)",
                pdryvel == "2.0",
                f"found {pdryvel}",
            )
    if specnum == 40:
        check(
            f"{case_id} SPECIES_040.PROVENANCE.txt present",
            (outdir / "SPECIES" / "SPECIES_040.PROVENANCE.txt").is_file(),
        )
    check(f"{case_id} METEO_ARGS.txt present", (outdir / "METEO_ARGS.txt").is_file())
    check(f"{case_id} INPUT_DERIVATION.json present", (outdir / "INPUT_DERIVATION.json").is_file())


def _as_u32_list(value, length: int):
    if not isinstance(value, list) or len(value) != length:
        return None
    out = []
    for item in value:
        if not isinstance(item, int) or item < 0 or item >= 2**32:
            return None
        out.append(item)
    return out


def candidate_philox_identity(case_id: str, case: dict):
    """Canonical v2 Philox identity reader (no silent fallback, no v1).

    Returns (base_key, base_counter, count, deterministic, derivation, error).
    derivation is one of the versioned executable contract values.
    Unknown or missing derivations fail closed.
    """
    if case.get("schema_version") != 2 or "version" in case:
        return None, None, None, False, None, (
            f"case {case_id}: unsupported schema version; only schema_version 2 "
            "is accepted (v1 is frozen, see MIGRATION_NOTES.md)"
        )
    stochastic = case.get("stochastic")
    if not isinstance(stochastic, dict):
        return None, None, None, False, None, (
            f"case {case_id}: stochastic must be an object"
        )
    for field in ("candidate_philox", "oracle_seed"):
        if field not in stochastic:
            return None, None, None, False, None, (
                f"case {case_id}: stochastic.{field} is required explicitly; "
                "use null when that model has no stochastic identity"
            )
    cand = stochastic["candidate_philox"]
    if cand is None:
        if case_id == "ADV-ANA-001":
            return None, None, 1, True, None, None
        return None, None, None, False, None, (
            f"case {case_id}: missing stochastic.candidate_philox; "
            "no default key substituted"
        )
    base_key = _as_u32_list(cand.get("base_key"), 2)
    base_counter = _as_u32_list(cand.get("base_counter"), 4)
    count = cand.get("count")
    derivation = cand.get("derivation")
    if base_key is None:
        return None, None, None, False, None, (
            f"case {case_id}: stochastic.candidate_philox.base_key malformed"
        )
    if base_counter is None:
        return None, None, None, False, None, (
            f"case {case_id}: stochastic.candidate_philox.base_counter malformed"
        )
    if not isinstance(count, int) or isinstance(count, bool) or count <= 0:
        return None, None, None, False, None, (
            f"case {case_id}: stochastic.candidate_philox.count must be > 0"
        )
    if derivation not in SUPPORTED_PHILOX_DERIVATIONS:
        return None, None, None, False, None, (
            f"case {case_id}: unsupported stochastic.candidate_philox.derivation "
            f"{derivation!r}; supported={sorted(SUPPORTED_PHILOX_DERIVATIONS)}"
        )
    if "identical_repeats" in cand:
        return None, None, None, False, None, (
            f"case {case_id}: legacy stochastic.candidate_philox.identical_repeats "
            "is forbidden; encode semantics in derivation"
        )
    return base_key, base_counter, count, False, derivation, None


def expected_philox_for_seed(case_id: str, base_key, base_counter, seed_index: int,
                             derivation: str):
    """Derive one key/counter exactly from the declared versioned policy."""
    if derivation == PHILOX_DERIVATION_REUSE_BASE_IDENTITY_V1:
        return list(base_key), list(base_counter)
    if derivation == PHILOX_DERIVATION_WRAPPING_ADD_KEY0_V1:
        return [(base_key[0] + seed_index) % 2**32, base_key[1]], list(base_counter)
    raise ValueError(
        f"case {case_id}: unsupported Philox derivation {derivation!r}"
    )


def audit_candidate_case(case_id: str, case: dict, case_dir: Path) -> None:
    seeds = sorted(case_dir.glob("seed_*.json"))
    release = case["release"]
    expected_count = int(release["particle_count"])
    expected_mass = GEN.case_total_mass_kg(case_id, case)
    base_key, base_counter, ensemble_count, deterministic, derivation, identity_error = (
        candidate_philox_identity(case_id, case)
    )
    if identity_error is not None:
        check(f"{case_id} Philox identity declared", False, identity_error)
    else:
        check(f"{case_id} Philox identity declared", True)
    if case_id == "ADV-ANA-001":
        check(f"{case_id} exactly one deterministic seed", len(seeds) == 1, f"found {len(seeds)}")
    elif case_id == "REPEAT-009":
        check(f"{case_id} exactly two repeat seeds", len(seeds) == 2, f"found {len(seeds)}")
        if len(seeds) == 2:
            a = json.loads(seeds[0].read_text(encoding="utf-8"))
            b = json.loads(seeds[1].read_text(encoding="utf-8"))
            check(f"{case_id} repeats share one Philox key", a.get("philox_key") == b.get("philox_key"))
    else:
        check(f"{case_id} seed files present", len(seeds) >= 1, "no seed_*.json")
    for path in seeds:
        seed = json.loads(path.read_text(encoding="utf-8"))
        stem = f"{case_id}/{path.name}"
        check(f"{stem} particle_count", seed.get("particle_count") == expected_count,
              f"{seed.get('particle_count')} vs {expected_count}")
        initial = seed.get("metrics", {}).get("initial_mass_kg")
        check(f"{stem} initial mass", initial is not None and math.isclose(initial, expected_mass, rel_tol=1e-12),
              f"{initial} vs {expected_mass}")
        if deterministic:
            pass
        elif identity_error is not None:
            check(f"{stem} Philox derivation", False, identity_error)
        else:
            idx = seed.get("seed_index", 0)
            if not isinstance(idx, int) or idx < 0 or idx >= ensemble_count:
                check(f"{stem} seed_index in declared ensemble", False,
                      f"seed_index {seed.get('seed_index')} outside [0, {ensemble_count})")
            else:
                expected_key, expected_counter = expected_philox_for_seed(
                    case_id, base_key, base_counter, idx, derivation
                )
                check(f"{stem} Philox derivation", seed.get("philox_key") == expected_key,
                      f"{seed.get('philox_key')} vs {expected_key}")
                check(f"{stem} Philox counter", seed.get("philox_counter") == expected_counter,
                      f"{seed.get('philox_counter')} vs {expected_counter}")
        check(f"{stem} adapter recorded", bool(seed.get("adapter")))


def audit_oracle_case(case_id: str, fort_dir: Path, oracle_dir: Path, require: bool) -> None:
    summary = oracle_dir / case_id / "oracle_summary.json"
    if not summary.is_file():
        check(f"{case_id} oracle summary present", not require,
              "run scripts/run-corpus.sh oracle" if require else "oracle not run yet")
        return
    data = json.loads(summary.read_text(encoding="utf-8"))
    releases = (fort_dir / case_id / "RELEASES").read_text(encoding="utf-8")
    expected_g = float(GEN.namelist_value(releases, "MASS").replace("D", "E"))
    actual_g = float(data.get("release_mass_g", -1.0))
    check(f"{case_id} oracle summary release mass", math.isclose(actual_g, expected_g, rel_tol=1e-9),
          f"{actual_g} vs {expected_g}")
    # Decoder stores reservoirs under first_slice.weighted and last_slice.weighted
    # with keys: airborne, dry_deposited, wet_deposited
    for key in ("raw_files", "header", "first_slice", "last_slice"):
        check(f"{case_id} oracle summary has {key}", key in data)
    for slice_key in ("first_slice", "last_slice"):
        if slice_key in data:
            weighted = data[slice_key].get("weighted", {})
            for res_key in ("airborne", "dry_deposited", "wet_deposited"):
                check(f"{case_id} oracle {slice_key}.weighted has {res_key}", res_key in weighted)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--fixtures", default=str(REPO / "fixtures" / "corpus"))
    parser.add_argument("--candidate-dir", default=None)
    parser.add_argument("--oracle-dir", default=None)
    parser.add_argument("--require-oracle", action="store_true")
    args = parser.parse_args()

    fixtures = Path(args.fixtures)
    corpus_index = json.loads((fixtures / "corpus.json").read_text(encoding="utf-8"))
    fort_dir = fixtures / "fortran"
    cases = {c["id"]: c for c in corpus_index["cases"]}

    for case_id, entry in cases.items():
        if entry["status"] != "implemented" or case_id == "ETEX-MINI-013":
            continue
        case_path = fixtures / "cases" / f"{case_id}.json"
        if not case_path.is_file():
            if case_id == "RESTART-010":
                continue
            check(f"{case_id} case JSON present", False, f"missing {case_path}")
            continue
        case = json.loads(case_path.read_text(encoding="utf-8"))
        if case_id not in ("RESTART-010", "REPEAT-009"):
            # RESTART-010 is oracle-only documentation; REPEAT-009 reuses the
            # neutral case on the candidate with no oracle counterpart (no
            # oracle seed control exists).
            audit_fixture_case(case_id, case, fort_dir)

    if args.candidate_dir:
        candidate_dir = Path(args.candidate_dir)
        for case_id, entry in cases.items():
            if entry["status"] != "implemented" or case_id in ("ETEX-MINI-013", "RESTART-010"):
                continue
            case_path = fixtures / "cases" / f"{case_id}.json"
            if not case_path.is_file():
                continue
            case = json.loads(case_path.read_text(encoding="utf-8"))
            case_dir = candidate_dir / case_id
            if not case_dir.is_dir():
                check(f"{case_id} candidate output present", False, "no seed files")
                continue
            audit_candidate_case(case_id, case, case_dir)

    if args.oracle_dir or args.require_oracle:
        oracle_dir = Path(args.oracle_dir) if args.oracle_dir else None
        if oracle_dir is not None:
            for case_id, entry in cases.items():
                if entry["status"] != "implemented" or case_id in ("ETEX-MINI-013", "RESTART-010", "REPEAT-009"):
                    continue
                audit_oracle_case(case_id, fort_dir, oracle_dir, args.require_oracle)

    if FAILURES:
        print(f"\n{len(FAILURES)} input-equality check(s) FAILED:")
        for failure in FAILURES:
            print(f"  - {failure}")
        raise SystemExit(1)
    print("\nCorpus input audit: all checks passed.")


if __name__ == "__main__":
    main()
