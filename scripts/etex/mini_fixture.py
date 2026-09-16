#!/usr/bin/env python3
"""Pack or verify the independent ERA5 subset used by the ETEX mini run."""

import argparse
import hashlib
import json
import zipfile
from pathlib import Path


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def pack(source, archive, manifest):
    import numpy as np

    metadata_path = source / "metadata.json"
    if not metadata_path.is_file():
        raise ValueError("source lacks ERA5 arrays or metadata")
    metadata = json.loads(metadata_path.read_text(encoding="utf-8"))
    if not metadata["variables"]:
        raise ValueError("ERA5 metadata has no variables")
    for entry in metadata["variables"]:
        values = np.load(source / entry["file"], mmap_mode="r", allow_pickle=False)
        if list(values.shape) != entry["shape"] or str(values.dtype) != entry["dtype"]:
            raise ValueError(f"ERA5 shape or type mismatch: {entry['file']}")
        if not np.all(np.isfinite(values)):
            raise ValueError(f"ERA5 array has non-finite values: {entry['file']}")
    names = {"metadata.json", "times.npy", "latitudes.npy", "longitudes.npy"}
    names.update(entry["file"] for entry in metadata["variables"])
    files = [source / name for name in sorted(names)]
    if any(not path.is_file() for path in files):
        raise ValueError("source lacks an ERA5 array listed in metadata")
    archive.parent.mkdir(parents=True, exist_ok=True)
    members = {}
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED,
                         compresslevel=9) as destination:
        for path in files:
            data = path.read_bytes()
            info = zipfile.ZipInfo(path.name, (1980, 1, 1, 0, 0, 0))
            info.compress_type = zipfile.ZIP_DEFLATED
            destination.writestr(info, data, compress_type=zipfile.ZIP_DEFLATED,
                                 compresslevel=9)
            members[path.name] = sha256(data)
    record = {"archive_sha256": sha256(archive.read_bytes()),
              "members_sha256": members}
    manifest.write_text(json.dumps(record, indent=2) + "\n", encoding="utf-8")


def unpack(archive, manifest, output):
    record = json.loads(manifest.read_text(encoding="utf-8"))
    if sha256(archive.read_bytes()) != record["archive_sha256"]:
        raise ValueError("ETEX mini archive hash mismatch")
    output.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(archive) as source:
        if set(source.namelist()) != set(record["members_sha256"]):
            raise ValueError("ETEX mini archive member list mismatch")
        for name, expected in record["members_sha256"].items():
            if Path(name).name != name:
                raise ValueError("unsafe archive member")
            data = source.read(name)
            if sha256(data) != expected:
                raise ValueError(f"ETEX mini member hash mismatch: {name}")
            (output / name).write_bytes(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("pack", "unpack"))
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--manifest", type=Path, required=True)
    parser.add_argument("--source", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.mode == "pack":
        if args.source is None:
            parser.error("pack requires --source")
        pack(args.source, args.archive, args.manifest)
    else:
        if args.output is None:
            parser.error("unpack requires --output")
        unpack(args.archive, args.manifest, args.output)


if __name__ == "__main__":
    main()
