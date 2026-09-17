#!/usr/bin/env python3
"""Generate versioned Fortran oracle inputs for the Issue #6 test corpus.

Reads fixtures/corpus/cases/*.json and writes
fixtures/corpus/fortran/<CASE>/{COMMAND,RELEASES,OUTGRID,AGECLASSES,RECEPTORS}
plus a METEO.txt record of the synthetic-GRIB generator call. SPECIES files
are copied verbatim from the pinned upstream checkout (never edited).

Usage:
    python3 scripts/corpus/generate_fortran_fixtures.py --flexpart-dir ../flexpart
"""

import argparse
import json
import shutil
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
CORPUS = REPO / "fixtures" / "corpus"
CASES = CORPUS / "cases"
FORTRAN_OUT = CORPUS / "fortran"

OUTGRID_TEXT = """&OUTGRID
 OUTLON0=       9.50,
 OUTLAT0=       8.50,
 NUMXGRID=         32,
 NUMYGRID=         32,
 DXOUT=          0.10,
 DYOUT=          0.10,
 OUTHEIGHTS=  100.0, 250.0, 500.0, 750.0, 1000.0, 1500.0, 2000.0, 2500.0, 3000.0, 5000.0,
 /
"""

AGECLASS_1H = """&AGECLASS
 NAGECLASS= 1,
 LAGE= 3600,
 /
"""

RECEPTORS_ZERO = """*******************************************************************************
*                                                                             *
*  Input file for the Lagrangian particle dispersion model FLEXPART           *
*                        Please specify receptor points                       *
*******************************************************************************
                0
"""

COMMAND_TEMPLATE = """&COMMAND
 LDIRECT=               1,
 IBDATE=         20240101,
 IBTIME=           000000,
 IEDATE=         20240101,
 IETIME=           010000,
 LOUTSTEP=           1800,
 LOUTAVER=           1800,
 LOUTSAMPLE=           300,
 ITSPLIT=        99999999,
 LSYNCTIME=            300,
 CTL=            {ctl:.7f},
 IFINE=                 4,
 IOUT=                  1,
 IPOUT=                 2,
 LSUBGRID=              0,
 NXSHIFT=               0,
 LNETCDFOUT=            0,
 LCONVECTION=           {lconvection},
 LAGESPECTRA=           0,
 IPIN=                  {ipin},
 IOUTPUTFOREACHRELEASE= 1,
 IFLUX=                 0,
 MDOMAINFILL=           0,
 IND_SOURCE=            1,
 IND_RECEPTOR=          1,
 MQUASILAG=             0,
 NESTED_OUTPUT=         0,
 LINIT_COND=            0,
 SURF_ONLY=             0,
 CBLFLAG=               0,
 OHFIELDS_PATH= "../../flexin/",
 LTURBULENCE=           {lturbulence},
 /
"""

RELEASES_TEMPLATE = """&RELEASES_CTRL
 NSPEC      =           1,
 SPECNUM_REL=          {specnum},
 /
&RELEASE
 IDATE1  =       20240101,
 ITIME1  =         000000,
 IDATE2  =       20240101,
 ITIME2  =         000000,
 LON1    =         10.000,
 LON2    =         10.000,
 LAT1    =         10.000,
 LAT2    =         10.000,
 Z1      =         50.000,
 Z2      =         50.000,
 ZKIND   =              1,
 MASS    =       1.0000E0,
 PARTS   =       {parts:10d},
 COMMENT =    "{comment}",
 /
"""


def meteo_call(case_id: str, case: dict) -> str:
    wind = case.get("wind", {})
    surface = case.get("surface", {})
    u = wind.get("u_m_s", wind.get("u0_m_s", 5.0))
    v = wind.get("v_m_s", 0.0)
    shear = 0.004 if case_id == "WIND-SHEAR-003" else 0.0
    sshf = float(surface.get("sensible_heat_flux_w_m2", 40.0))
    blh = float(surface.get("mixing_height_m", 1500.0))
    lsp = float(surface.get("precip_large_scale_mm_h", 0.0))
    cp = float(surface.get("precip_convective_mm_h", 0.0))
    if case_id == "WET-008":
        lsp, cp = 2.0, 1.0
    if case_id in ("DRY-007", "ADV-ANA-001"):
        lsp, cp = 0.0, 0.0
    return (
        "python3 scripts/generate_synthetic_grib.py "
        f"--output-dir target/corpus/meteo/{case_id} --nx 32 --ny 32 --nz 12 "
        f"--u-wind {u} --v-wind {v} --w-wind 0.0 "
        f"--u-shear-per-m {shear} --sshf {sshf} --blh {blh} "
        f"--lsp {lsp} --cp {cp} --start-date 20240101 --hours 3"
    )


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
    parts = {
        "ADV-ANA-001": 1024,
        "WIND-UNI-002": 1000,
        "WIND-SHEAR-003": 1000,
        "PBL-STABLE-004": 1000,
        "PBL-NEUTRAL-005": 500,
        "PBL-UNSTABLE-006": 1000,
        "DRY-007": 500,
        "WET-008": 500,
        "RESTART-010": 500,
    }
    for case_id in desired:
        case_path = CASES / f"{case_id}.json"
        if case_path.is_file():
            case = json.loads(case_path.read_text(encoding="utf-8"))
        else:
            case = {}
        outdir = FORTRAN_OUT / case_id
        (outdir / "SPECIES").mkdir(parents=True, exist_ok=True)
        if case_id == "ADV-ANA-001":
            lturb, lconv, ctl, ipin = 0, 0, 5.0, 0
        elif case_id == "RESTART-010":
            lturb, lconv, ctl, ipin = 1, 0, 5.0, 0
        else:
            lturb, lconv, ctl, ipin = 1, 0, 5.0, 0
        (outdir / "COMMAND").write_text(
            COMMAND_TEMPLATE.format(
                ctl=ctl, lconvection=lconv, ipin=ipin, lturbulence=lturb
            ),
            encoding="utf-8",
        )
        specnum = 40 if case_id in ("DRY-007", "WET-008") else 24
        (outdir / "RELEASES").write_text(
            RELEASES_TEMPLATE.format(
                specnum=specnum, parts=parts[case_id], comment=case_id
            ),
            encoding="utf-8",
        )
        (outdir / "OUTGRID").write_text(OUTGRID_TEXT, encoding="utf-8")
        (outdir / "AGECLASSES").write_text(AGECLASS_1H, encoding="utf-8")
        (outdir / "RECEPTORS").write_text(RECEPTORS_ZERO, encoding="utf-8")
        species_src = aerosol if specnum == 40 else tracer
        if species_src.is_file():
            shutil.copyfile(outdir / "SPECIES" / f"SPECIES_{specnum:03d}", species_src) if False else shutil.copyfile(
                species_src, outdir / "SPECIES" / f"SPECIES_{specnum:03d}"
            )
        (outdir / "METEO.txt").write_text(
            meteo_call(case_id, case) + "\n", encoding="utf-8"
        )
        if case_id == "RESTART-010":
            (outdir / "RESTART-NOTE.txt").write_text(
                "Oracle-only restart illustration: rerun with IPIN=1 and LOUTRESTART set "
                "after a first run that wrote restart_* files (restart_mod.f90). "
                "No paired candidate restart exists (candidate has no restart API).\n",
                encoding="utf-8",
            )
        print(f"wrote {outdir}")
    print("Fortran corpus fixtures generated.")


if __name__ == "__main__":
    main()
