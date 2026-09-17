#!/usr/bin/env python3
"""Generate versioned Fortran oracle inputs for the Issue #6 test corpus.

Every oracle release/grid setting is derived from the matching candidate
case JSON under fixtures/corpus/cases/*.json so that both programs start
from demonstrably equal inputs:

- OUTGRID (OUTLON0/OUTLAT0/NUMXGRID/NUMYGRID/DXOUT/DYOUT) mirrors the case
  ``domain`` exactly.
- RELEASES (LON/LAT/Z/PARTS) mirrors the case ``release`` exactly.
- RELEASES MASS is the case total mass converted from kilograms (candidate
  unit) to grams (FLEXPART unit): ``MASS_g = mass_kg * 1000``.
- COMMAND dates follow the case ``integration`` window; turbulence,
  convection and timestepping switches follow ``oracle_command_overrides``.
- METEO_ARGS.txt records the exact synthetic-GRIB generator flags derived
  from the case wind/surface entries (single reference path:
  scripts/generate_synthetic_grib.py).

The inert tracer SPECIES is copied verbatim from the pinned upstream
checkout. The depositing SPECIES is derived from the upstream aerosol
example by dropping exactly the PNDIA key unknown to the v11.1 namelist
(see depositing_species_text), with provenance recorded per case. Each case
directory also records INPUT_DERIVATION.json with the source values and
conversion applied, and the generator re-parses every written file and
fails loudly when a derived value drifts from the case.

Usage:
    python3 scripts/corpus/generate_fortran_fixtures.py --flexpart-dir ../flexpart
"""

import argparse
import hashlib
import json
import re
import shutil
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
CORPUS = REPO / "fixtures" / "corpus"
CASES = CORPUS / "cases"
FORTRAN_OUT = CORPUS / "fortran"

# FLEXPART RELEASES MASS is in grams; candidate case masses are in kilograms.
KG_TO_G = 1000.0

# Standard concentration output levels [m] shared by all synthetic corpus
# cases. The output grid is independent of the wind-field levels; one fixed
# set keeps oracle comparison grids identical across cases.
STANDARD_OUTHEIGHTS = [
    100.0, 250.0, 500.0, 750.0, 1000.0,
    1500.0, 2000.0, 2500.0, 3000.0, 5000.0,
]

RECEPTORS_ZERO = """*******************************************************************************
*                                                                             *
*  Input file for the Lagrangian particle dispersion model FLEXPART           *
*                        Please specify receptor points                       *
*******************************************************************************
                0
"""


def case_total_mass_kg(case: dict) -> float:
    """Total released mass [kg] for a case JSON release block."""
    release = case["release"]
    if "mass_kg_total" in release:
        return float(release["mass_kg_total"])
    return float(release["particle_count"]) * float(
        release.get("mass_kg_per_particle", 1.0)
    )


def sim_end_date(start: str, total_s: int) -> tuple:
    """Derive (IEDATE, IETIME) from the YYYYMMDDHHMMSS start plus seconds.

    All corpus cases start at 2024-01-01 00:00:00 and run whole hours.
    """
    assert start == "20240101000000", f"unexpected corpus start: {start}"
    hours, rem = divmod(total_s, 3600)
    minutes, seconds = divmod(rem, 60)
    assert hours < 100, f"corpus run exceeds COMMAND date arithmetic: {total_s}s"
    return 20240101, hours * 10000 + minutes * 100 + seconds


def outgrid_text(case: dict) -> str:
    """OUTGRID namelist derived from the case domain block."""
    domain = case["domain"]
    heights = ", ".join(f"{h:6.1f}" for h in STANDARD_OUTHEIGHTS) + ","
    return (
        "&OUTGRID\n"
        f" OUTLON0=   {domain['xlon0_deg']:7.2f},\n"
        f" OUTLAT0=   {domain['ylat0_deg']:7.2f},\n"
        f" NUMXGRID=   {domain['nx']:7d},\n"
        f" NUMYGRID=   {domain['ny']:7d},\n"
        f" DXOUT=     {domain['dx_deg']:7.2f},\n"
        f" DYOUT=     {domain['dy_deg']:7.2f},\n"
        f" OUTHEIGHTS=  {heights}\n"
        " /\n"
    )


def command_text(case: dict) -> str:
    """COMMAND namelist derived from case integration and switch overrides."""
    integration = case["integration"]
    overrides = case.get("oracle_command_overrides", {})
    iedate, ietime = sim_end_date(
        integration.get("start", "20240101000000"), int(integration["total_s"])
    )
    ctl = float(overrides.get("CTL", 5.0))
    ifine = int(overrides.get("IFINE", 4))
    lturbulence = int(overrides.get("LTURBULENCE", 1))
    lconvection = int(overrides.get("LCONVECTION", 0))
    return (
        "&COMMAND\n"
        " LDIRECT=               1,\n"
        " IBDATE=         20240101,\n"
        " IBTIME=           000000,\n"
        f" IEDATE=         {iedate},\n"
        f" IETIME=           {ietime:06d},\n"
        " LOUTSTEP=           1800,\n"
        " LOUTAVER=           1800,\n"
        " LOUTSAMPLE=           300,\n"
        " ITSPLIT=        99999999,\n"
        " LSYNCTIME=            300,\n"
        f" CTL=            {ctl:.7f},\n"
        f" IFINE=                 {ifine},\n"
        " IOUT=                  1,\n"
        " IPOUT=                 2,\n"
        " LSUBGRID=              0,\n"
        " NXSHIFT=               0,\n"
        " LNETCDFOUT=            0,\n"
        f" LCONVECTION=           {lconvection},\n"
        " LAGESPECTRA=           0,\n"
        " IPIN=                  0,\n"
        " IOUTPUTFOREACHRELEASE= 1,\n"
        " IFLUX=                 0,\n"
        " MDOMAINFILL=           0,\n"
        " IND_SOURCE=            1,\n"
        " IND_RECEPTOR=          1,\n"
        " MQUASILAG=             0,\n"
        " NESTED_OUTPUT=         0,\n"
        " LINIT_COND=            0,\n"
        " SURF_ONLY=             0,\n"
        " CBLFLAG=               0,\n"
        ' OHFIELDS_PATH= "../../flexin/",\n'
        f" LTURBULENCE=           {lturbulence},\n"
        " /\n"
    )


def releases_text(case_id: str, case: dict, specnum: int) -> str:
    """RELEASES namelist derived from the case release block.

    FLEXPART MASS is in grams; the candidate works in kilograms.
    """
    release = case["release"]
    mass_g = case_total_mass_kg(case) * KG_TO_G
    return (
        "&RELEASES_CTRL\n"
        " NSPEC      =           1,\n"
        f" SPECNUM_REL=          {specnum},\n"
        " /\n"
        "&RELEASE\n"
        " IDATE1  =       20240101,\n"
        " ITIME1  =         000000,\n"
        " IDATE2  =       20240101,\n"
        " ITIME2  =         000000,\n"
        f" LON1    =     {release['lon_deg']:8.3f},\n"
        f" LON2    =     {release['lon_deg']:8.3f},\n"
        f" LAT1    =     {release['lat_deg']:8.3f},\n"
        f" LAT2    =     {release['lat_deg']:8.3f},\n"
        f" Z1      =     {release['z_m']:9.3f},\n"
        f" Z2      =     {release['z_m']:9.3f},\n"
        " ZKIND   =              1,\n"
        f" MASS    =       {mass_g:.4E},\n"
        f" PARTS   =       {int(release['particle_count']):10d},\n"
        f' COMMENT =    "{case_id}",\n'
        " /\n"
    )


def ageclass_text(case: dict) -> str:
    """Single age class covering the full integration window."""
    total_s = int(case["integration"]["total_s"])
    return f"&AGECLASS\n NAGECLASS= 1,\n LAGE= {total_s},\n /\n"


def meteo_args(case_id: str, case: dict) -> str:
    """Exact synthetic-GRIB generator flags derived from case wind/surface."""
    wind = case.get("wind", {})
    surface = case.get("surface", {})
    u = wind.get("u_m_s", wind.get("u0_m_s", 5.0))
    v = wind.get("v_m_s", 0.0)
    shear = float(wind.get("u_shear_per_s", 0.0))
    sshf = float(surface.get("sensible_heat_flux_w_m2", 40.0))
    blh = float(surface.get("mixing_height_m", 1500.0))
    lsp = float(surface.get("precip_large_scale_mm_h", 0.0))
    cp = float(surface.get("precip_convective_mm_h", 0.0))
    if case_id == "WET-008":
        lsp, cp = 2.0, 1.0
    if case_id in ("DRY-007", "ADV-ANA-001"):
        lsp, cp = 0.0, 0.0
    return (
        f"--nx 32 --ny 32 --nz 12 --u-wind {u} --v-wind {v} --w-wind 0.0 "
        f"--u-shear-per-m {shear} --sshf {sshf} --blh {blh} "
        f"--lsp {lsp} --cp {cp} --start-date 20240101 --hours 3"
    )


def sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def set_namelist_key(text: str, key: str, value: str) -> tuple:
    """Replace the assignment of KEY in namelist text; fail if absent/ambiguous."""
    pattern = re.compile(rf"(?im)^(?P<indent>\s*){key}\s*=\s*[^,/\n]+(?P<tail>,?)")
    matches = list(pattern.finditer(text))
    if len(matches) != 1:
        raise SystemExit(
            f"expected exactly one {key} assignment, found {len(matches)}; "
            "upstream example changed, re-derive deliberately"
        )
    match = matches[0]
    return (
        text[: match.start()] + f"{match.group('indent')}{key}={value}{match.group('tail')}" + text[match.end():],
        match.group(0).strip(),
    )


def species_for_case(case_id: str, tracer: Path, aerosol: Path) -> tuple:
    """Derive the oracle SPECIES file for a deposition case.

    - DRY-007 starts from the inert tracer example (no wet removal, no
      aerosol settling) and sets ``PDRYVEL=2.0``. FLEXPART converts PDRYVEL
      from cm/s to m/s (``readoptions_mod.f90``: ``dryvel*0.01``), giving a
      constant 0.02 m/s dry deposition velocity with the same
      ``exp(-vdep*|dt|/(2*href))`` removal law and sub-``2*href`` layer gate
      as the candidate (``drydepo_mod.f90:860``,
      ``physics/deposition.rs:dry_deposition_probability_step``,
      ``href = 15 m`` both sides). This equates the dry process intensity
      by experimental design; the comparison stays diagnostic.
    - WET-008 starts from the upstream aerosol example (wet scavenging
      parameters active) and drops exactly the ``PNDIA`` key that the pinned
      v11.1 ``species_params`` namelist does not declare (the oracle aborts
      with ``SPECIES file not in NAMELIST format`` otherwise). FLEXPART
      computes scavenging rates dynamically from precipitation and cloud
      fields, so no single-coefficient equivalence with the candidate
      uniform ``lambda`` exists; the wet comparison stays diagnostic, and
      the oracle aerosol additionally dry-deposits (reported separately in
      the budget, never equated with the candidate dry-zero path).

    Returns (file_text, provenance_text).
    """
    if case_id == "DRY-007":
        base = tracer.read_text(encoding="utf-8")
        text, old = set_namelist_key(base, "PDRYVEL", "2.0")
        provenance = (
            f"Derived from upstream examples/Tracer/SPECIES/{tracer.name} "
            f"at sha256 {sha256_file(tracer)} "
            "(pinned oracle reference/flexpart-11.1.json).\n"
            f"Edited assignment: `{old}` -> `PDRYVEL=2.0` (2.0 cm/s = 0.02 m/s, "
            "exactly the candidate DRY-007 dry_deposition_velocity_m_s; wet "
            "removal stays disabled as in the tracer).\n"
        )
        header = (
            "! Corpus-derived dry-deposition SPECIES for DRY-007.\n"
            "! See SPECIES_040.PROVENANCE.txt for the exact upstream source and edit.\n"
        )
        return header + text, provenance
    raw_lines = aerosol.read_text(encoding="utf-8").splitlines(keepends=True)
    kept, removed = [], []
    for line in raw_lines:
        code = re.sub(r"!.*", "", line)
        if re.match(r"\s*PNDIA\s*=", code, re.IGNORECASE):
            removed.append(line)
        else:
            kept.append(line)
    if not removed:
        raise SystemExit(
            f"{aerosol}: expected PNDIA line not found; upstream example changed, "
            "re-derive the depositing SPECIES deliberately"
        )
    provenance = (
        f"Derived from upstream examples/Aerosol/SPECIES/{aerosol.name} "
        f"at sha256 {sha256_file(aerosol)} "
        "(pinned oracle reference/flexpart-11.1.json).\n"
        "Removed lines (PNDIA is not a member of the v11.1 species_params "
        "namelist; the oracle aborts on unknown keys):\n"
        + "".join(f"  {line}" if line.endswith("\n") else f"  {line}\n" for line in removed)
    )
    header = (
        "! Corpus-derived depositing SPECIES for WET-008.\n"
        "! See SPECIES_040.PROVENANCE.txt for the exact upstream source and edit.\n"
    )
    return header + "".join(kept), provenance


def namelist_value(text: str, key: str) -> str:
    """Extract one raw value for KEY from namelist text (comments stripped)."""
    stripped = re.sub(r"!.*", "", text)
    match = re.search(
        rf"\b{re.escape(key)}\s*=\s*([^,/\n]+)", stripped, re.IGNORECASE
    )
    if not match:
        raise ValueError(f"key {key} not found in namelist")
    return match.group(1).strip().strip("\"'")


def verify_case(case_id: str, case: dict, outdir: Path, specnum: int) -> None:
    """Re-parse every written fixture and fail loudly on any drift."""
    release = case["release"]
    domain = case["domain"]
    failures = []

    def check(name: str, actual, expected=True, tolerance: float = 0.0) -> None:
        if isinstance(expected, float):
            ok = abs(actual - expected) <= tolerance
        else:
            ok = actual == expected
        if not ok:
            failures.append(f"{name}: fixture has {actual!r}, case needs {expected!r}")

    releases = (outdir / "RELEASES").read_text(encoding="utf-8")
    check("RELEASES LON1", float(namelist_value(releases, "LON1")), float(release["lon_deg"]), 1e-9)
    check("RELEASES LAT1", float(namelist_value(releases, "LAT1")), float(release["lat_deg"]), 1e-9)
    check("RELEASES Z1", float(namelist_value(releases, "Z1")), float(release["z_m"]), 1e-9)
    check("RELEASES PARTS", int(namelist_value(releases, "PARTS")), int(release["particle_count"]))
    check("RELEASES SPECNUM_REL", int(namelist_value(releases, "SPECNUM_REL")), specnum)
    expected_g = case_total_mass_kg(case) * KG_TO_G
    actual_g = float(namelist_value(releases, "MASS").replace("D", "E"))
    # MASS is written with %.4E (5 significant digits).
    check("RELEASES MASS_g", actual_g, expected_g, 1e-4 * expected_g)

    outgrid = (outdir / "OUTGRID").read_text(encoding="utf-8")
    check("OUTGRID OUTLON0", float(namelist_value(outgrid, "OUTLON0")), float(domain["xlon0_deg"]), 1e-9)
    check("OUTGRID OUTLAT0", float(namelist_value(outgrid, "OUTLAT0")), float(domain["ylat0_deg"]), 1e-9)
    check("OUTGRID NUMXGRID", int(namelist_value(outgrid, "NUMXGRID")), int(domain["nx"]))
    check("OUTGRID NUMYGRID", int(namelist_value(outgrid, "NUMYGRID")), int(domain["ny"]))
    check("OUTGRID DXOUT", float(namelist_value(outgrid, "DXOUT")), float(domain["dx_deg"]), 1e-9)
    check("OUTGRID DYOUT", float(namelist_value(outgrid, "DYOUT")), float(domain["dy_deg"]), 1e-9)

    command = (outdir / "COMMAND").read_text(encoding="utf-8")
    overrides = case.get("oracle_command_overrides", {})
    check("COMMAND LTURBULENCE", int(namelist_value(command, "LTURBULENCE")), int(overrides.get("LTURBULENCE", 1)))
    check("COMMAND LCONVECTION", int(namelist_value(command, "LCONVECTION")), int(overrides.get("LCONVECTION", 0)))
    check("COMMAND CTL", float(namelist_value(command, "CTL")), float(overrides.get("CTL", 5.0)), 1e-6)

    species_text = (outdir / "SPECIES" / f"SPECIES_{specnum:03d}").read_text(encoding="utf-8")
    code = re.sub(r"!.*", "", species_text)
    check(
        f"SPECIES_{specnum:03d} has no PNDIA (unknown to v11.1)",
        re.search(r"(?im)^\s*PNDIA\s*=", code) is None,
    )
    if specnum == 40:
        check(
            "SPECIES_040.PROVENANCE.txt present",
            (outdir / "SPECIES" / "SPECIES_040.PROVENANCE.txt").is_file(),
        )
    if case_id == "DRY-007":
        # 2.0 cm/s = 0.02 m/s, exactly the candidate dry velocity.
        check(
            "DRY SPECIES PDRYVEL=2.0",
            float(namelist_value(species_text, "PDRYVEL")) == 2.0,
        )

    if failures:
        raise SystemExit(f"{case_id}: derived fixtures drift from case JSON:\n" + "\n".join(failures))


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--flexpart-dir", default=str(REPO.parent / "flexpart"))
    args = parser.parse_args()
    flexpart = Path(args.flexpart_dir)
    tracer = flexpart / "examples" / "Tracer" / "SPECIES" / "SPECIES_024"
    aerosol = flexpart / "examples" / "Aerosol" / "SPECIES" / "SPECIES_040"
    if not tracer.is_file():
        raise SystemExit(f"upstream tracer species not found: {tracer}")

    desired = [
        "ADV-ANA-001",
        "WIND-UNI-002",
        "WIND-SHEAR-003",
        "PBL-STABLE-004",
        "PBL-NEUTRAL-005",
        "PBL-UNSTABLE-006",
        "DRY-007",
        "WET-008",
        "RESTART-010",
    ]
    for case_id in desired:
        case_path = CASES / f"{case_id}.json"
        if case_path.is_file():
            case = json.loads(case_path.read_text(encoding="utf-8"))
        else:
            # RESTART-010 documents the oracle-only restart path and reuses
            # the neutral release/grid shape.
            case = json.loads((CASES / "PBL-NEUTRAL-005.json").read_text(encoding="utf-8"))
        outdir = FORTRAN_OUT / case_id
        (outdir / "SPECIES").mkdir(parents=True, exist_ok=True)
        specnum = 40 if case_id in ("DRY-007", "WET-008") else 24
        (outdir / "COMMAND").write_text(command_text(case), encoding="utf-8")
        (outdir / "RELEASES").write_text(
            releases_text(case_id, case, specnum), encoding="utf-8"
        )
        (outdir / "OUTGRID").write_text(outgrid_text(case), encoding="utf-8")
        (outdir / "AGECLASSES").write_text(ageclass_text(case), encoding="utf-8")
        (outdir / "RECEPTORS").write_text(RECEPTORS_ZERO, encoding="utf-8")
        if specnum == 40:
            if not aerosol.is_file():
                raise SystemExit(f"upstream aerosol species not found: {aerosol}")
            text, provenance = species_for_case(case_id, tracer, aerosol)
            (outdir / "SPECIES" / "SPECIES_040").write_text(text, encoding="utf-8")
            (outdir / "SPECIES" / "SPECIES_040.PROVENANCE.txt").write_text(
                provenance, encoding="utf-8"
            )
        else:
            shutil.copyfile(tracer, outdir / "SPECIES" / "SPECIES_024")
        args_line = meteo_args(case_id, case)
        (outdir / "METEO_ARGS.txt").write_text(args_line + "\n", encoding="utf-8")
        (outdir / "METEO.txt").write_text(
            "python3 scripts/generate_synthetic_grib.py "
            f"--output-dir target/corpus/meteo/{case_id} {args_line}\n",
            encoding="utf-8",
        )
        derivation = {
            "case_file": f"fixtures/corpus/cases/{case_path.name}",
            "release_lon_deg": case["release"]["lon_deg"],
            "release_lat_deg": case["release"]["lat_deg"],
            "release_z_m": case["release"]["z_m"],
            "particle_count": case["release"]["particle_count"],
            "candidate_mass_kg": case_total_mass_kg(case),
            "mass_conversion": "MASS_g = mass_kg * 1000 (FLEXPART MASS is in grams)",
            "oracle_mass_g": case_total_mass_kg(case) * KG_TO_G,
            "outgrid_from_domain": {
                key: case["domain"][key]
                for key in ("xlon0_deg", "ylat0_deg", "nx", "ny", "dx_deg", "dy_deg")
            },
            "outheights_m": STANDARD_OUTHEIGHTS,
            "outheights_note": "Standard concentration output levels shared by all "
            "synthetic cases; independent of wind-field levels.",
        }
        (outdir / "INPUT_DERIVATION.json").write_text(
            json.dumps(derivation, indent=2) + "\n", encoding="utf-8"
        )
        if case_id == "RESTART-010":
            (outdir / "RESTART-NOTE.txt").write_text(
                "Oracle-only restart illustration: rerun with IPIN=1 and LOUTRESTART set "
                "after a first run that wrote restart_* files (restart_mod.f90). "
                "No paired candidate restart exists (candidate has no restart API).\n",
                encoding="utf-8",
            )
        verify_case(case_id, case, outdir, specnum)
        print(f"wrote and verified {outdir}")
    print("Fortran corpus fixtures generated and input-equal to case JSON.")


if __name__ == "__main__":
    main()
