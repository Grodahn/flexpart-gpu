#!/usr/bin/env python3
"""Retrieve the small native-level ERA5 field set documented in native-mini."""

import argparse
import hashlib
import json
import os
from pathlib import Path


REQUEST = {
    "date": "1994-10-23",
    "time": "15/18/21",
    "levelist": "1/to/137",
    "levtype": "ml",
    "param": "130/131/132/133/135",
    "stream": "oper",
    "type": "an",
    "area": "53/-8/43/8",
    "grid": "0.25/0.25",
    "format": "grib",
}
DATASET = "reanalysis-era5-complete"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path,
                        default=Path("target/etex/native_era5/era5-19941023-151821-ml.grib"))
    args = parser.parse_args()
    if args.output.exists():
        parser.error(f"output already exists: {args.output}")
    try:
        import cdsapi
    except ImportError:
        parser.error("install cdsapi to retrieve ERA5 Complete")

    key = os.environ.get("CDS_API")
    client = (cdsapi.Client(url="https://cds.climate.copernicus.eu/api",
                            key=key, quiet=True, progress=False)
              if key else cdsapi.Client(quiet=True, progress=False))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    try:
        client.retrieve(DATASET, REQUEST, str(args.output))
    except Exception as error:
        message = str(error).replace(key, "[REDACTED]") if key else str(error)
        raise RuntimeError(f"CDS retrieval failed: {message}") from None
    digest = hashlib.sha256(args.output.read_bytes()).hexdigest()
    print(json.dumps({"file": str(args.output), "bytes": args.output.stat().st_size,
                      "sha256": digest}, indent=2))


if __name__ == "__main__":
    main()
