# Stage A early 11.1 measurements — 21 October 2026

Measured candidate: `3c4c17c6ce75e9a7f3e821494045378cfab43b80`. Production sources remain at the pre-#197/pre-#198 baseline.

**Scientific verdict: NON_COMPARABLE. No speedup is calculated.**

Hardware: Intel Core i7-8650U (4 cores / 8 threads), Intel UHD Graphics 620, Vulkan, Windows display driver 31.0.101.2115. Reference: pristine FLEXPART 11.1 `c70586c2b7f5258850705325881c61f557ea9bd8`, gfortran 11.4.0, `FC=gfortran eta=no arch=x86-64 -j4`, single-thread frozen OpenMP profile.

Reference image: `sha256:df8b65783487ada74b485f3173b760e220b39bc2f40a2ce9668ebe3895e3a137`. Reference executable: `4b79cdf8dbcd1d7ac5362e27cc00dac1aa7717b88f6d8cbe0b79580d41e242cf`.

| Regime | Particles | Runs | Mean s | Median s | Stddev s | Min–max s |
|---|---:|---:|---:|---:|---:|---:|
| canonical-production | 100,000 | 5/5 | 2.436 | 2.082 | 0.766 | 1.739–3.540 |
| pinned-11.1-fortran | 100,000 | 5/5 | 21.126 | 17.403 | 13.016 | 12.720–44.080 |
| canonical-production | 1,000,000 | 5/5 | 18.760 | 16.759 | 4.618 | 15.291–26.378 |
| pinned-11.1-fortran | 1,000,000 | 5/5 | 86.940 | 80.250 | 24.175 | 67.649–127.719 |

Mean canonical throughput was 529,469 particle-steps/s at 100,000 particles, 666,487 particle-steps/s at 1,000,000 particles. Fortran throughput is null because its actual integration-call count is not exposed.

Canonical timing covers 12 actual production steps × 300 s after at least 3 s same-driver warm-up, including strict per-step particle readback, profiling synchronization and invariant audits. Fortran timing covers the full original executable (ingestion, integration, output) inside Docker. The scopes and physics differ. Both request the same point release, 10 m/s uniform U, zero V/W, one kg per particle, and 300 s synchronization; the native/global meteorology and active turbulence/output semantics remain different.

All candidate samples retain finite, active particles, exact unchanged species mass, and no top/horizontal contact. Actual Fortran runtime counters confirm the requested spawned/live counts with zero terminations. Existing raw/decoded oracle outputs are retained. At each count, all five reference concentration-file hash sets are identical. This is reference repeatability evidence, not a paired canonical scientific verdict.

Five repetitions alternate engine order. Process one includes cold startup effects; later processes reuse OS/driver/image caches. The noticeable variation is retained in the distribution rather than selecting a best sample. GPU warm-up step count varies with wall time; each run records exact measured epochs and step count.

Per-run JSON receipts, compiler/build/hardware provenance, phase timings, artifact hashes and invariant observations: [frozen measurements](../../fixtures/benchmarks/results/stage-a-20261021.json). Full raw logs, source encodings, particle bytes, Fortran inputs/outputs and aggregate reports remain in `target/early-benchmark/stage-a-final/` in the benchmark worktree.

Reproduce from the measured commit (Python 3.11+, hardware GPU, Docker and pristine reference):

```text

python scripts/benchmark_early.py --checkout target/flexpart-reference --output target/early-benchmark/stage-a-rerun

```

The output directory must be new. [Methodology and exact limitations](early-11.1.md) and [versioned manifest](../../fixtures/benchmarks/early-11.1-v1.json) define the two diagnostic workloads. Existing Fortran phase timers overlap; no transport-only or separate concentration-serialization cost is inferred. GPU timestamps, VRAM peak and GPU host peak memory are unavailable.

Validation: eight Python harness tests; one Rust invariant test; formatting and Clippy pass (existing repository warnings remain); navigation checker passes; full serial Cargo suite passes 730 tests. The initial parallel Windows software-WGSL run crashed in unchanged accumulation code; that target and the full suite pass serially. Both real canonical driver regressions pass on hardware in normal, separated validation and compaction modes. Production scientific sources, tolerances, #197/#198 and #46/#116 are unchanged.

The preliminary `0b0231a` trial stopped after a Python 3.10 worker import failure. Its retained timing is not part of this series. The final series uses the corrected clean commit above.

A future accepted scientific comparison still needs a matched canonical turbulence/advection envelope, common meteorology and output acceptance. #197/#198 require their own separately versioned rerun and do not inherit this baseline’s verdict. Issue #199 stays open for review of the diagnostic evidence; this PR does not close any scientific parity or production adoption issue.

Provenance marker: **GPT6.1 Sol**.
