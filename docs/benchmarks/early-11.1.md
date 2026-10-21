# Early pinned 11.1 benchmark (#199)

The versioned [Stage A manifest](../../fixtures/benchmarks/early-11.1-v1.json)
defines two **non-comparable** diagnostic workloads. No speedup is calculated.
This measures the real canonical production path without changing physics.
Provenance marker: **GPT6.1 Sol**.

The [21 October 2026 measured report](early-11.1-stage-a-20261021.md) and
[frozen per-run receipts](../../fixtures/benchmarks/results/stage-a-20261021.json)
retain five successful hardware/reference repetitions at both counts.

## Path audit and scientific boundary

| Regime | Executed path | Scientific scope |
|---|---|---|
| `legacy-diagnostic` | `fortran-validation`, `bench-timeloop`, Criterion timestep group and `compare-fortran.sh`: `canonical: None`, `run_legacy_diagnostic_timestep` | Historical synthetic diagnostic; old March reports are not current production performance. The scientific validation report used 10.4, not pinned 11.1. |
| `canonical-production` | `bench-canonical`: #173 `prepare_canonical_meteorology`, retained `CanonicalMeteorologySlot` / resource Arc, actual `ForwardTimeLoopDriver::run_timestep`, #76/#171 resident queries | Current #112 native equal-column U/V/center-W path, GPU PBL, fused Hanna/Langevin, zero deposition/decay forcing; no convection. Strict per-step particle synchronization and profiling enabled. |
| `pinned-11.1-fortran` | Clean manifest-pinned upstream full `FLEXPART` executable, existing frozen Docker build/runtime profile, generated `ADV-ANA-001` options | Advection-only with `LTURBULENCE=0`, `LCONVECTION=0`, inert `SPECIES_024`; full ingestion, integration and concentration output. |

The pinned `advance_mod.f90:250,385,551` checks `lturbulence`; Fortran can
disable turbulence. Current `forward/operators.rs` always encodes Hanna/Langevin,
and `ForwardTimeLoopConfig` exposes no turbulence switch. The existing acceptance
case cannot therefore become a canonical advection-only workload through benchmark
configuration. Using a stochastic Fortran case would additionally require its
scientific production acceptance and matched meteorology. That work belongs to
the scientific owners; this harness does not invent tolerances or a paired verdict.

Other finite mismatches: regional 32x32x3 native canonical inputs versus the
existing 32x32x12 global synthetic Fortran GRIB profile; fixed release-latitude canonical horizontal velocity conversion; instantaneous particle state versus time-averaged
concentration. Both request 100k or 1M particles, one kg each, at 10 E / 50 N and 100 m AGL,
with uniform 10 m/s U, zero V/W and a 300 s synchronization interval,
but matching those quantities alone does not establish scientific equivalence.

Stage A uses invariant native tops and zero mean W. Every synchronized particle
state is inspected: finite coordinates/turbulence, all particles active, exact
unchanged species mass, strict interior horizontal location and `0 < z < top`.
With zero W, current and predicted transport heights equal the preceding audited
height. Any output contact rejects the run. These finite invariant checks are
execution diagnostics, not a new scientific trajectory tolerance. #198's
inclusive top action is neither implemented nor claimed. #197/#198 source changes
are rejected by the Stage A launcher; a new series/manifest must own later runs.

## Reproduce

Use Python 3.11+, Cargo, Docker Desktop/Linux and a pristine pinned reference
checkout with LF source bytes. Read [reference setup](../reference-environment.md).
The candidate checkout must be clean and committed. The default reference path is
`target/flexpart-reference`; `--checkout` selects another pristine checkout.

```text
python scripts/benchmark_early.py --checkout <pristine-flexpart> --output target/early-benchmark/stage-a-first
python scripts/test_benchmark_early.py
```

The output directory must be new and under this checkout's `target/`. The launcher
builds the release binary once and delegates Fortran build verification to
`oracle_build_cache.py`; repeated runs reuse only verified build artifacts.
Cargo's machine-readable artifact receipt selects the executable even when a
target directory or target triple is configured. The launcher freezes the exact
manifest bytes in `scenario.json` and uses that copy for every candidate run.
`generate_fortran_fixtures.py` derives each count's options and the kg-to-g mass
conversion, and `generate_synthetic_grib.py` prepares meteorology. No upstream
source or particle cap is patched. Immutable Docker image, compiler/packages,
build flags, binary and object hashes are retained. The pristine checkout and
binary are verified again after actual Fortran execution.
Prepared inputs must match their generation hashes before each reference run;
consumed options, pathnames and meteorology must remain unchanged during it.

Five independent processes per engine/count alternate order. Each GPU process
warms the same driver for at least three wall seconds and two timesteps; 12
subsequent steps represent one simulated hour. Warm-up trajectories continue into
measurement, so exact epoch/step counts are recorded and this is explicitly not
the Fortran release-to-end interval. Fortran repetition one is the process-cold
sample; later independent processes reuse OS/image caches. These labels do not
assert flushed driver/disk caches. No best-sample speedup is reported.

The launcher fixes GPU PBL, fused production mode, compaction off and hardware
selection; inherited FLEXPART/WGPU overrides are removed. Actual adapter class,
driver/backend, source hashes, Philox identity, candidate commit/tree, compiler
and executable hashes bind each campaign. Software WGSL is rejected for timing.

## Timing and artifacts

`report.json` and `report.md` contain aggregate distributions, successful/requested
sample counts, per-run errors, immutable provenance, scientific blockers and a
null speedup. Each sample retains raw logs, phase JSON, full candidate particle
bytes and SHA-256, generated Fortran options/meteorology hashes, runtime guard,
raw concentration output and the existing decoded oracle summary. Missing
build/device/output/decoder evidence yields nonzero failure, with results retained.
Candidate receipts must also contain the configured warm-up and measured epochs,
complete finite phase timings, mass/height invariants and matching raw byte length.

Candidate wall time includes production encoding/submission, profiling waits,
per-step D2H and invariant audit. Driver initialization, canonical meteorology
reconstruction/upload, cold first step, warm-up and final artifact write/hash are
separate. Existing step timers are host wall intervals: submit-to-poll is not a
GPU timestamp or an isolated transport kernel measurement. The raw
`submit_to_poll_ms` field is the blocking poll duration after submission; encoding
can already overlap GPU work, so it is not a full device execution interval.
Status readbacks and
per-step particles are explicit host consumers. Transfer descriptions and the
three particle-buffer memory lower bound are not measured bandwidth/VRAM peaks.
Unexposed GPU timestamp, peak VRAM and host peak memory are null.

Fortran model wall time is measured inside the container around the genuine full
executable, including meteorology decode and output; host Docker invocation wall
is separate. Generation/build/decode logs are separate. Linux child peak RSS is
available, and the pinned executable's `SYSTEM_CLOCK` diagnostic timers for wind
reading, time manager, first timestep and particle-file writing are retained.
Those scopes overlap and are not a disjoint phase sum; particle-file time excludes
other concentration serialization. Actual particle integration-call count is not, so Fortran particle-steps/s
is null instead of assuming the adaptive timestep denominator. No transport-only,
transfer-only or serialization-only Fortran timer is claimed.

Existing #112 invariant/top/device tests and existing pinned interpolation gates
remain the scientific owners. These measurements do not close #46/#116 or claim
production readiness or full FLEXPART parity. The issue remains open if repeated
hardware/reference evidence or required regressions cannot execute.
