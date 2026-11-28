#!/usr/bin/env python3
"""Replay the pinned ERA5 Complete ETEX pressure or model-level retrieval."""

import argparse
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path


SOURCE_ROOT = Path(__file__).resolve().parents[2] / "fixtures/etex/native-mini"
MODEL_REQUESTS = {"23": "request.json", "24": "request-next-day.json"}
CDS_API_URL = "https://cds.climate.copernicus.eu/api"


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
    pinned = model
    if not args.replay_model:
        manifest = model["file"].replace("-ml.grib", "-lnsp.request.json")
        pinned = json.loads((SOURCE_ROOT / manifest).read_bytes())
    output = args.output_dir / pinned["file"]
    record = output.with_suffix(".request.json")
    pending = output.with_suffix(".grib.part")
    if any(path.exists() for path in (output, record, pending)):
        parser.error("retrieve into a fresh destination")
    args.output_dir.mkdir(parents=True, exist_ok=True)
    pending.touch(exist_ok=False)

    try:
        client = cdsapi.Client(
            url=CDS_API_URL, key=os.environ.get("CDS_API") or None, verify=True,
            quiet=True, progress=False, retry_max=2, timeout=60,
        )
        client.retrieve(pinned["dataset"], pinned["request"], str(pending))
    except Exception as error:
        # Exception strings from clients and transports can include credentials.
        status = getattr(getattr(error, "response", None), "status_code", None)
        parser.exit(1, f"CDS retrieval failed ({type(error).__name__}, HTTP {status})\n")

    with pending.open("rb") as response:
        raw = response.read(pinned["bytes"] + 1)
    digest = hashlib.sha256(raw).hexdigest()
    if len(raw) != pinned["bytes"] or digest != pinned["sha256"]:
        parser.exit(1, f"CDS response does not match the pinned size and SHA-256 for {output.name}\n")
    pending.rename(output)
    record.write_bytes((json.dumps({
        "source": pinned["source"],
        "dataset": pinned["dataset"],
        "api_url": CDS_API_URL,
        "retrieved_utc": datetime.now(timezone.utc).isoformat(timespec="seconds"),
        "request": pinned["request"],
        "file": output.name,
        "bytes": len(raw),
        "sha256": digest,
        "model_request_sha256": hashlib.sha256(model_request_bytes).hexdigest(),
        "model_sha256": model["sha256"],
    }, indent=2) + "\n").encode("utf-8"))


if __name__ == "__main__":
    main()
