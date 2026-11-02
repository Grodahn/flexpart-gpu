"""Bounded research-only ECMWF IFS index/range capture; never a production IO API."""

import argparse
import datetime as dt
import hashlib
import json
from pathlib import Path
import subprocess

ROOT = "https://data.ecmwf.int/forecasts"
MAX_INDEX_BYTES = 2_000_000
MAX_MESSAGE_BYTES = 20_000_000
MAX_TOTAL_BYTES = 160_000_000


def digest(data):
    return hashlib.sha256(data).hexdigest()


def capture(url, path, limit, byte_range=None):
    """Use the system CA store; refuse oversized or ignored range responses."""
    headers = path.with_suffix(path.suffix + ".headers")
    command = ["curl.exe", "--fail", "--silent", "--show-error", "--max-time", "120",
               "--max-filesize", str(limit), "--dump-header", str(headers), "--output", str(path)]
    if byte_range:
        command += ["--range", byte_range]
    subprocess.run(command + [url], check=True)
    data = path.read_bytes()
    header_text = headers.read_text()
    if len(data) > limit:
        raise ValueError("response exceeds capture bound")
    if byte_range and (" 206 " not in header_text or
                       f"bytes {byte_range}/" not in header_text):
        raise ValueError("server did not honor exact byte range")
    return {"url": url, "path": path.name, "bytes": len(data), "sha256": digest(data),
            "retrieved_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
            "response_headers": header_text, "range": byte_range}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--cycle", required=True, help="YYYYMMDDHH; explicit coherent 00/12 cycle")
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--messages", action="store_true", help="Fetch fixed representative selection")
    args = parser.parse_args()
    cycle = dt.datetime.strptime(args.cycle, "%Y%m%d%H")
    if cycle.hour not in (0, 12):
        raise ValueError("select 00 or 12 UTC")
    if args.output.exists() and any(args.output.iterdir()):
        raise ValueError("capture directory must be empty; never overwrite archived evidence")
    args.output.mkdir(parents=True, exist_ok=True)
    manifest = {"cycle": args.cycle, "model": "ifs", "stream": "oper", "type": "fc",
                "indexes": [], "messages": [], "maximum_message_bytes": MAX_TOTAL_BYTES}
    used = 0
    for resolution, suffixes in (("0p1", ("sfc", "pl")), ("0p25", ("",))):
        for step in range(0, 49, 3):
            for suffix in suffixes:
                stem = f"{args.cycle}0000-{step}h-oper-fc" + (f"-{suffix}" if suffix else "")
                url = f"{ROOT}/{cycle:%Y%m%d}/{cycle:%H}z/ifs/{resolution}/oper/{stem}"
                index_path = args.output / f"{resolution}-{stem}.index"
                item = capture(url + ".index", index_path, MAX_INDEX_BYTES)
                item.update(resolution=resolution, step=step, suffix=suffix)
                manifest["indexes"].append(item)
                rows = [json.loads(line) for line in index_path.read_text().splitlines()]
                for row in rows:
                    # Fixed small selection: vertical identity, surface, precipitation sequence, control.
                    selected = (resolution == "0p1" and (
                        (step == 0 and row.get("param") in ("t", "u", "v", "w", "q", "gh") and str(row.get("levelist")) == "850") or
                        (step == 0 and row.get("levtype") == "sfc" and row.get("param") in
                         ("2t", "sp", "2d", "10u", "10v", "lsm", "z", "sd", "tcc")) or
                        (step in (0, 3, 6, 24, 48) and row.get("param") in ("tp", "cp", "lsp")) or
                        (step == 3 and row.get("param") in ("ewss", "nsss", "ssr", "sf")) or
                        (step == 3 and row.get("param") == "t" and str(row.get("levelist")) == "850"))) or (
                        resolution == "0p25" and step in (0, 3) and row.get("param") in ("2t", "tp"))
                    if not args.messages or not selected:
                        continue
                    start, length = int(row["_offset"]), int(row["_length"])
                    used += length
                    if start < 0 or length <= 0 or length > MAX_MESSAGE_BYTES or used > MAX_TOTAL_BYTES:
                        raise ValueError("fixed capture budget exceeded")
                    path = args.output / f"{resolution}-{step}-{row['param']}-{row.get('levelist', 'sfc')}.grib2"
                    message = capture(url + ".grib2", path, length, f"{start}-{start + length - 1}")
                    if message["bytes"] != length:
                        raise ValueError("truncated range")
                    message.update(resolution=resolution, step=step, selector=row)
                    manifest["messages"].append(message)
                (args.output / "retrieval-manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", newline="\n")
    print(json.dumps({"indexes": len(manifest["indexes"]), "messages": len(manifest["messages"]), "message_bytes": used}))


if __name__ == "__main__":
    main()
