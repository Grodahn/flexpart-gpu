#!/usr/bin/env python3
"""Retrieve the six native-level ERA5 ETEX snapshots and eta velocity."""

import argparse
import hashlib
import json
import os
from pathlib import Path


BASE_REQUEST = {
    "levelist": "1/to/137",
    "levtype": "ml",
    "stream": "oper",
    "type": "an",
    "area": "53/-8/43/8",
    "grid": "0.25/0.25",
    "format": "grib",
}
DATASET = "reanalysis-era5-complete"
REQUESTS = (
    ("1994-10-23", "15/18/21", "130/131/132/133/135",
     "era5-19941023-151821-ml.grib"),
    ("1994-10-24", "00/03/06", "130/131/132/133/135",
     "era5-19941024-000306-ml.grib"),
    ("1994-10-23", "15/18/21", "77",
     "era5-19941023-151821-etadot.grib"),
    ("1994-10-24", "00/03/06", "77",
     "era5-19941024-000306-etadot.grib"),
)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output-dir", type=Path,
                        default=Path("target/etex/native_era5"))
    args = parser.parse_args()
    try:
        import cdsapi
    except ImportError:
        parser.error("install cdsapi to retrieve ERA5 Complete")

    key = os.environ.get("CDS_API")
    client = (cdsapi.Client(url="https://cds.climate.copernicus.eu/api",
                            key=key, quiet=True, progress=False)
              if key else cdsapi.Client(quiet=True, progress=False))
    args.output_dir.mkdir(parents=True, exist_ok=True)
    for date, time, param, filename in REQUESTS:
        output = args.output_dir / filename
        if output.exists():
            parser.error(f"output already exists: {output}")
        request = {**BASE_REQUEST, "date": date, "time": time, "param": param}
        try:
            client.retrieve(DATASET, request, str(output))
        except Exception as error:
            message = str(error).replace(key, "[REDACTED]") if key else str(error)
            raise RuntimeError(f"CDS retrieval failed: {message}") from None
        digest = hashlib.sha256(output.read_bytes()).hexdigest()
        print(json.dumps({"file": str(output), "request": request,
                          "bytes": output.stat().st_size,
                          "sha256": digest}, indent=2))


if __name__ == "__main__":
    main()
