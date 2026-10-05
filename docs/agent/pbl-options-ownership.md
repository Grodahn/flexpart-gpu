# Shared PBL options ownership (#147)

This is an API ownership refactor under [issue #147](https://github.com/Grodahn/flexpart-gpu/issues/147).
It introduces no scientific, numerical, execution, profile-schema or validation
change. CPU preparation remains in IO; #117 cleanup and #124 decomposition are
outside this contract.

## Ownership and callers

At baseline `b430aa5053539c985435e9296faa3759a888f7ab`,
[IO preparation](../../src/io/pbl_params.rs) defined the eight-field
`PblComputationOptions` and its `Default`; [IO](../../src/io/mod.rs) re-exported it.
The production/control-plane callers were [GPU PBL encoding](../../src/gpu/pbl.rs),
[forward/backward driver configuration](../../src/simulation/timeloop/config.rs)
and [candidate profile conversion](../../src/validation/candidate_physics.rs).
CPU preparation's cell/grid/clamp functions, its colocated tests,
[advection benchmarks](../../benches/advection.rs) and
[CPU/GPU comparison diagnostics](../../tests/cpu_gpu_comparison.rs) also used it.
These diagnostic callers retain their established IO imports.

The canonical path is now `pbl::PblComputationOptions` at the
[PBL facade](../../src/pbl/mod.rs), with exactly one struct/default implementation
in the [private options owner](../../src/pbl/options.rs). Both
`io::PblComputationOptions` and `io::pbl_params::PblComputationOptions` explicitly
re-export that same type. The three enumerated production/control-plane callers
now import the PBL facade. CPU preparation consumes the same nominal type.
`TimeBoundsBehavior`, the remaining IO preparation API and all calculations
retain their owners. Only the two existing roughness/reference-height constants
needed by CPU preparation are re-exported internally; no public constants API
or additional IO preparation facade is introduced.

## Preservation proof

[Public contract regressions](../../tests/pbl_options_contract.rs) passed against
the baseline before moving the owner, freezing all eight `f32::to_bits()` values
in declaration order:

| Field | Frozen default bits |
| --- | --- |
| `roughness_length_m` | `0x3dcc_cccd` |
| `wind_reference_height_m` | `0x4120_0000` |
| `heat_flux_neutral_threshold_w_m2` | `0x3f80_0000` |
| `bulk_richardson_critical` | `0x3e80_0000` |
| `min_shear_squared_m2_s2` | `0x3e80_0000` |
| `fallback_mixing_height_m` | `0x4448_0000` |
| `hmix_min_m` | `0x42c8_0000` |
| `hmix_max_m` | `0x458c_a000` |

The final tests compare all three paths to that baseline, assert matching
`TypeId`s, construct options through all three paths in one homogeneous array,
and pass them to existing forward/backward configuration and CPU grid preparation.
The GPU regression requires an adapter, dispatches the existing kernel through
all three paths and compares their device results exactly. The frozen profile
and a distinct-value mapping probe cover every field. This verifies API and
behavior preservation; it does not establish new scientific parity.

## Representative source selection

Use #147's read-first convention: complete selected UTF-8 source files, including
colocated tests, with CRLF normalized to LF; KiB means bytes divided by 1024.
Navigation/contract documents and separate verification files are not counted.

| Shared-options inspection | Selected source files | Lines | Bytes | KiB |
| --- | --- | ---: | ---: | ---: |
| Before | `src/pbl/mod.rs` + `src/io/pbl_params.rs` | 1,284 | 45,559 | 44.5 |
| After | `src/pbl/mod.rs` + `src/pbl/options.rs` | 379 | 13,101 | 12.8 |

This reduces the representative source selection by 905 lines and 32,458 bytes
(71.2%). Callers can find the shared contract through
[PBL navigation](repo-map.md#pbl-turbulence) without reading unrelated CPU IO
preparation algorithms. This measurement concerns source selection only, not
execution time, scientific accuracy or a claim of minimal possible context.
The focused and broader preservation checks are listed in the
[test entry](test-map.md#pbl-turbulence); final outcomes and retained artifacts
belong to the implementing PR.
