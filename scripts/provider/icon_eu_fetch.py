#!/usr/bin/env python3
"""Fetch the bounded #230 ICON-EU audit sample from DWD Open Data, fail closed.

Research tooling for issue #230 only. It is not an operational retrieval or
ingestion path (that is #32-B). The script expands a versioned sample request
into exact DWD file names, downloads each ``.grib2.bz2`` file once into an
empty output directory, decompresses it next to the archive, and writes a
retrieval manifest with the URL, HTTP metadata, sizes, SHA-256 of the
compressed and decompressed bytes, and the UTC download time.

DWD Open Data is a rolling archive: a URL that works today expires within
about a day. Re-running after expiry fails closed (non-zero exit) instead of
substituting another cycle. ``--expect`` re-checks downloaded bytes against a
previously frozen manifest, so a later run can only reproduce the audit with
byte-identical inputs (for example from a private archive mirror passed via
``--base-url``).

Usage::

    python -I scripts/provider/icon_eu_fetch.py \
        --request fixtures/meteorology/icon-eu-audit-v1/sample-request.json \
        --output <new-empty-dir> [--expect <frozen retrieval manifest>]
"""

import argparse
import bz2
import datetime
import hashlib
import json
import os
import sys
import urllib.error
import urllib.request

REQUEST_SCHEMA = "flexpart-gpu.icon-eu-sample-request"
MANIFEST_SCHEMA = "flexpart-gpu.icon-eu-retrieval-manifest"
PRODUCT_PREFIX = "icon-eu_europe_regular-lat-lon"
KINDS = ("time-invariant", "model-level", "pressure-level", "single-level")
TIMEOUT_SECONDS = 120


class FetchError(RuntimeError):
    """A sample could not be retrieved or verified; the audit input is incomplete."""


def expand_levels(levels):
    """Return explicit level numbers from a list or an inclusive ``"a-b"`` range."""
    if levels is None:
        return [None]
    if isinstance(levels, str):
        first, last = (int(part) for part in levels.split("-"))
        return list(range(first, last + 1))
    return [int(level) for level in levels]


def file_name(kind, cycle, variable, step, level):
    """Build the exact DWD file name; the name is never used as scientific evidence."""
    parts = [PRODUCT_PREFIX, kind, cycle]
    if kind != "time-invariant":
        parts.append(f"{step:03d}")
    if level is not None:
        parts.append(str(level))
    parts.append(variable)
    return "_".join(parts) + ".grib2.bz2"


def expand_request(request):
    """Expand the request into ``(url, relative path)`` pairs, rejecting malformed groups."""
    if request.get("schema") != REQUEST_SCHEMA or request.get("version") != 1:
        raise FetchError("unsupported sample-request schema/version")
    base_url = request["base_url"].rstrip("/")
    entries = []
    for group in request["groups"]:
        kind = group["kind"]
        if kind not in KINDS:
            raise FetchError(f"unknown file kind {kind!r}")
        cycle = group.get("cycle", request["cycle"])
        if len(cycle) != 10 or not cycle.isdigit():
            raise FetchError(f"malformed cycle {cycle!r}")
        steps = [None] if kind == "time-invariant" else group["steps"]
        for variable in group["variables"]:
            for step in steps:
                for level in expand_levels(group.get("levels")):
                    name = file_name(kind, cycle, variable, step, level)
                    url = f"{base_url}/{cycle[8:10]}/{variable.lower()}/{name}"
                    entries.append((url, os.path.join(cycle, name)))
    if len({path for _, path in entries}) != len(entries):
        raise FetchError("sample request expands to duplicate files")
    return entries


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def download(url):
    """Return body and HTTP metadata; any HTTP/transport error or short read is fatal."""
    try:
        with urllib.request.urlopen(url, timeout=TIMEOUT_SECONDS) as response:
            body = response.read()
            length = response.headers.get("Content-Length")
            last_modified = response.headers.get("Last-Modified")
            status = response.status
    except (urllib.error.URLError, OSError) as error:
        raise FetchError(f"{url}: {error}") from error
    if status != 200 or (length is not None and int(length) != len(body)):
        raise FetchError(f"{url}: status {status}, length {length} vs {len(body)}")
    return body, last_modified


def fetch(entries, output):
    records = []
    for url, relative in entries:
        archive_path = os.path.join(output, relative)
        os.makedirs(os.path.dirname(archive_path), exist_ok=True)
        body, last_modified = download(url)
        try:
            grib = bz2.decompress(body)
        except (OSError, ValueError) as error:
            raise FetchError(f"{url}: invalid bz2 stream: {error}") from error
        with open(archive_path, "wb") as handle:
            handle.write(body)
        with open(archive_path[: -len(".bz2")], "wb") as handle:
            handle.write(grib)
        records.append({
            "url": url,
            "path": relative.replace(os.sep, "/"),
            "last_modified": last_modified,
            "bz2_bytes": len(body),
            "bz2_sha256": sha256(body),
            "grib2_bytes": len(grib),
            "grib2_sha256": sha256(grib),
            "downloaded_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(timespec="seconds"),
        })
    return records


def compare_with_expected(records, expected_path):
    """Fail unless every retrieved file is byte-identical to the frozen manifest."""
    with open(expected_path, encoding="utf-8") as handle:
        expected = {item["path"]: item for item in json.load(handle)["files"]}
    actual = {item["path"]: item for item in records}
    if set(expected) != set(actual):
        raise FetchError("retrieved file set differs from the frozen manifest")
    for path, item in actual.items():
        if item["grib2_sha256"] != expected[path]["grib2_sha256"]:
            raise FetchError(f"{path}: GRIB2 bytes differ from the frozen manifest")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--request", required=True)
    parser.add_argument("--output", required=True, help="new or empty directory")
    parser.add_argument("--base-url", help="override, e.g. a private byte-identical mirror")
    parser.add_argument("--expect", help="frozen retrieval manifest to verify against")
    args = parser.parse_args(argv)

    with open(args.request, encoding="utf-8") as handle:
        request = json.load(handle)
    if args.base_url:
        request["base_url"] = args.base_url
    if os.path.isdir(args.output) and os.listdir(args.output):
        raise FetchError(f"output directory {args.output} is not empty")
    os.makedirs(args.output, exist_ok=True)

    records = fetch(expand_request(request), args.output)
    if args.expect:
        compare_with_expected(records, args.expect)
    manifest = {
        "schema": MANIFEST_SCHEMA,
        "version": 1,
        "request_sha256": sha256(open(args.request, "rb").read()),
        "base_url": request["base_url"],
        "files": records,
    }
    with open(os.path.join(args.output, "retrieval-manifest.json"), "w", encoding="utf-8") as handle:
        json.dump(manifest, handle, indent=2)
        handle.write("\n")
    print(f"retrieved {len(records)} files into {args.output}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except FetchError as error:
        print(f"icon_eu_fetch: FAILED: {error}", file=sys.stderr)
        sys.exit(1)
