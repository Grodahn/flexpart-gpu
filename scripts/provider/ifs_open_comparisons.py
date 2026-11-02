"""Research diagnostics on frozen IFS values; no field conversion is admitted."""

import argparse
import json
from pathlib import Path

import eccodes as ec
import numpy as np

import ifs_open_audit as audit


def comparisons(root):
    """Compare shared export nodes and expose negative cumulative differences."""
    manifest_bytes = (root / "retrieval-manifest.json").read_bytes()
    manifest = json.loads(manifest_bytes)
    audit.validate_inventory(root, manifest)
    messages = {item["path"]: item for item in manifest["messages"]}
    if len(messages) != len(manifest["messages"]):
        raise ValueError("duplicate comparison input")
    inputs = {}

    def values(name):
        if name not in messages:
            raise ValueError(f"missing comparison input: {name}")
        item = messages[name]
        data = (root / name).read_bytes()
        audit.validate_raw(data, item)
        handle = ec.codes_new_from_message(data)
        try:
            metadata = audit.read_metadata(handle)
            audit.validate_message(data, item, manifest, metadata)
            audit.compare_metadata(audit.octets.parse(data), metadata)
            shape = ec.codes_get(handle, "Nj"), ec.codes_get(handle, "Ni")
            decoded = ec.codes_get_values(handle).reshape(shape)
            if not np.isfinite(decoded).all():
                raise ValueError("nonfinite comparison values")
            inputs[name] = item["sha256"]
            return decoded
        finally:
            ec.codes_release(handle)

    report = {"same_cycle_control": [], "accumulation_diagnostics": [], "tp_minus_cp_diagnostics": []}
    for step, param in ((0, "2t"), (0, "tp"), (3, "tp")):
        fine = values(f"0p1-{step}-{param}-sfc.grib2")[::5, ::5]
        coarse = values(f"0p25-{step}-{param}-sfc.grib2")[::2, ::2]
        delta = fine - coarse
        report["same_cycle_control"].append({
            "step": step, "param": param, "shared_gridpoints": fine.size,
            "max_abs_difference": float(abs(delta).max()), "mean_abs_difference": float(abs(delta).mean()),
            "interpretation": "Different export grids/quantization; no required equality or input equivalence claim"})
    for param in ("tp", "cp"):
        for start, end in ((0, 3), (3, 6), (6, 24), (24, 48)):
            delta = values(f"0p1-{end}-{param}-sfc.grib2") - values(f"0p1-{start}-{param}-sfc.grib2")
            report["accumulation_diagnostics"].append({
                "parameter": param, "from": start, "to": end, "negative_count": int((delta < 0).sum()),
                "minimum_delta_m": float(delta.min()), "maximum_delta_m": float(delta.max()),
                "interpretation": "Only 0-3 and 3-6 are consecutive 3h intervals; larger windows are not 3h forcing"})
    for step in (0, 3, 6, 24, 48):
        delta = values(f"0p1-{step}-tp-sfc.grib2") - values(f"0p1-{step}-cp-sfc.grib2")
        report["tp_minus_cp_diagnostics"].append({
            "step": step, "negative_count": int((delta < 0).sum()), "minimum_m": float(delta.min()),
            "interpretation": "Diagnostic only; not an admitted LSP transformation"})
    report["source"] = {"cycle": manifest["cycle"], "model": manifest["model"],
                        "stream": manifest["stream"], "type": manifest["type"],
                        "manifest_sha256": audit.sha(manifest_bytes), "inputs": dict(sorted(inputs.items()))}
    return report


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("root", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.write_text(json.dumps(comparisons(args.root), indent=2) + "\n", newline="\n")
