# Original flagged gas-carrier transport-top fixture v2

Additive genuine public-advance evidence for #213, #203 and #214. Provenance marker: **GPT6.1 Sol**.
Original revision: `c70586c2b7f5258850705325881c61f557ea9bd8`.

The 64 public rows cover frozen #180 scenarios 1-8, both directions, both geometries
and both signed epochs. Every observed flag is true; every settling velocity is exactly
positive zero (f32 `00000000`). This supplies no GPU or complete production parity.

`output.txt` is raw observed original execution; `output-unwrapped.txt` is the raw
control execution. `report.json` is decoded output with raw bits, per-row verdicts and
first-build provenance. `scenarios.json` freezes inputs and expected field identities.
`reproducibility.json` retains two independently prepared original-build inventories,
call linkage, compiler/image/recipe identities and exact raw/decoded comparison.
`manifest.json` hashes all files and all historical v1 files. Full build artifacts/logs
are retained in each run directory and by the transport-top CI workflow.

The source, selected carrier and observation limits, all per-case final-height/nstop
results and exact #214 consumption instructions are in
[the research record](../../../docs/research/flagged-top-213.md).

Reproduce from the repository root with new output paths:

```text
python scripts/transport/run_flagged_top_boundary_oracle.py --checkout <pristine-pin> --output-dir target/flagged-first
python scripts/transport/run_flagged_top_boundary_oracle.py --checkout <pristine-pin> --output-dir target/flagged-second
python scripts/transport/compare_top_boundary_builds.py --first target/flagged-first --second target/flagged-second --output target/flagged-pair.json
python scripts/transport/verify_flagged_top_boundary_oracle.py --fixture fixtures/transport/top-boundary-v2
python scripts/transport/check_settling_stop_contract.py --checkout <pristine-pin>
python scripts/transport/test_top_boundary_oracle.py
python scripts/transport/test_check_settling_stop_contract.py
```

The baseline report retains `RECORDED` to identify its inspected capture. Both actual
execution reports must independently pass the full audit against it before paired
verification accepts them. Missing or record-only runs cannot satisfy reproducibility.
Do not edit expected values in place: a changed result requires a new version and
scientific explanation. Every v1 file remains byte-identical.

Pre-merge review hardened selector-stage, declared-matrix and decimal/raw-bit audits.
Both retained original executions were re-audited with that verifier. Only audit
provenance and its hashes were refreshed; raw outputs, decoded scientific values,
scenario inputs, driver/source bytes and independent build identities are unchanged.
The manifest records the prior capture identity for this audit-only revision.
