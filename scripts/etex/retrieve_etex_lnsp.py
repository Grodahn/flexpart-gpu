#!/usr/bin/env python3
"""Retrieve ERA5 Complete ETEX log surface pressure on the #237 analysis grid."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path


SOURCE_ROOT = Path(__file__).resolve().parents[2] / "fixtures/etex/native-mini"
MODEL_REQUESTS = {"23": "request.json", "24": "request-next-day.json"}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("day", choices=MODEL_REQUESTS)
    parser.add_argument("output_dir", type=Path)
    parser.add_argument("--replay-model", action="store_true",
                        help="replay the original #237 model-level request")
    args = parser.parse_args()
    try:
        import cdsapi
    except ImportError:
        parser.error("install cdsapi to retrieve ERA5 Complete")

    model_request_bytes = (SOURCE_ROOT / MODEL_REQUESTS[args.day]).read_bytes()
    model = json.loads(model_request_bytes)
    request = model["request"]
    filename = model["file"]
    if not args.replay_model:
        request = {**request, "levelist": "1", "param": "152"}
        filename = filename.replace("-ml.grib", "-lnsp.grib")
    output = args.output_dir / filename
    record = output.with_suffix(".request.json")
    pending = output.with_suffix(".grib.part")
    if any(path.exists() for path in (output, record, pending)):
        parser.error("retrieve into a fresh destination")
    args.output_dir.mkdir(parents=True, exist_ok=True)

    key = os.environ.get("CDS_API")
    try:
        options = {"quiet": True, "progress": False, "retry_max": 2, "timeout": 60}
        if key:
            options.update(url="https://cds.climate.copernicus.eu/api", key=key)
        client = cdsapi.Client(**options)
        client.retrieve(model["dataset"], request, str(pending))
    except Exception as error:
        # Exception strings from clients and transports can include credentials.
        status = getattr(getattr(error, "response", None), "status_code", None)
        parser.exit(1, f"CDS retrieval failed ({type(error).__name__}, HTTP {status})\n")

    raw = pending.read_bytes()
    if not raw.startswith(b"GRIB"):
        parser.exit(1, "CDS response does not start with a GRIB message\n")
    pending.rename(output)
    record.write_bytes((json.dumps({
        "source": model["source"],
        "dataset": model["dataset"],
        "retrieved_utc": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "request": request,
        "file": output.name,
        "bytes": len(raw),
        "sha256": hashlib.sha256(raw).hexdigest(),
        "model_request_sha256": hashlib.sha256(model_request_bytes).hexdigest(),
        "model_sha256": model["sha256"],
    }, indent=2) + "\n").encode("utf-8"))


if __name__ == "__main__":
    main()
