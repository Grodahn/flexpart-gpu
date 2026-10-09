# Verified build reuse (#176)

Contract: [issue #176](https://github.com/Grodahn/flexpart-gpu/issues/176).
Starting main: `6f60eab53808d91cb950704b10ab9e62755f92e0`, including #175.
Rebased onto `ebb114639fe19a550faeb66a4a1f659a1aedf099` after #112/#182 merged; its added production-advection CI step is retained.
Externally supplied provenance marker: **GPT6.1 Sol**.

## Build ownership and trust

`scripts/oracle_build_cache.py` extends #92's existing owner and cache directory.
Both `run-corpus.sh oracle` and Step 2 of `ci-gate.sh` call its `prepare` action.
Its v2 record identifies the pinned clean checkout, source/build-file bytes,
Docker recipe/compose/pin, exact `FC=gfortran eta=no arch=x86-64 -j4` arguments,
resolved immutable image, concrete compiler/linker/package identity, executable,
complete object/module set and full build transcript. Existing v1 records miss
and rebuild once. Records are atomically replaced after successful compilation
and cleanliness checks. Concurrent preparation fails explicitly; an interrupted
process may leave `target/oracle-cache/prepare.lock`, which must be removed only
after confirming no preparation is active.

Direct-driver reuse re-derives the complete parent identity and the running
compiler/linker/package identity; corrupted revision/flags/image/key fields cannot
be accepted by comparing a record to itself. Compilation and later Oracle
invocations bind the verified immutable Docker image ID, so retagging `latest`
cannot change their environment. A standalone container without an immutable
binding compiles uncached; a mismatched binding fails. Explicit alternate link
trees retain uncached legacy compilation without depending on an unrelated
parent record.

These are local, trusted build records. Oracle artifacts are never restored
from GitHub caches or accepted from PR uploads. A record does not authenticate
arbitrary externally supplied binaries. Missing/malformed records, changed
identities, missing or damaged binaries/objects/modules/logs rebuild or fail.

The vertical routine/conformance, interpolation, and W production drivers use
the same owner's build records, derived from the verified parent record plus
driver/recipe/flags and the retained binary/provenance artifacts. They retain
compilation only. Every synthetic/real routine invocation, all eight
interpolation cases, symbol/call-edge checks, output decoding, provenance and
fixture comparisons still execute. A standalone driver without a parent cache
retains its full compilation behavior and never claims a hit.
The two committed harness-source SHA-256 fields are updated to identify these
changed build scripts truthfully. All numerical fixture values, binary/output
hashes, schemas, comparisons, tolerances and scientific verdicts stay unchanged.

`scripts/ci-gate.sh --clean --particles 1000 --require-flex-extract-oracle`
forces a no-layer-cache Docker build, `make clean`, full Fortran compilation and
fresh direct-driver builds. The separate pinned flex_extract build remains
unchanged; it belongs to a different oracle revision/image/contract.

## Cargo caches

Both existing required jobs use the pinned Swatinem/rust-cache action. Its key
includes compiler/toolchain, OS/architecture, Cargo manifests/lockfiles and
configuration/environment. An explicit key adds runner image version and the
workflows' default-feature configuration. Changing features requires updating
that declared key. Debug/release caches remain separated by Cargo's profile
layout. Jobs remain separate and retain all existing #175 selections.

Archives explicitly include only `deps`, `build`, `.fingerprint` under the two
Cargo profiles, dependency checksum metadata, and the action's registry/git
caches. Workspace crates are excluded by the action. Scientific output trees,
GPU reports, oracle binaries and PASS results cannot enter these archives.
`cargo_cache_integrity.py` verifies the dependency artifact checksums before any
tests; missing/malformed/incomplete/damaged caches discard compilation artifacts
and rebuild normally. The checksum sidecar stays outside `target` so the action's
post-job target cleanup cannot delete it before publication. Required tests and GPU evidence checks always follow.
PR events can restore scoped caches but cannot save them. Main cannot read
feature/PR-scoped caches; trusted manual audit runs can save their own branch
cache without making it available to protected main.

## Measurements and limitations

Measured on Windows, Docker Desktop 29.8.0, GNU Fortran 11.4.0, with existing
Docker layers and downloaded Cargo dependencies retained. Full logs and JSON
measurements are under `target/issue176-evidence/` in the implementation worktree.
These are controlled local **preparation** measurements, not full scientific-gate
speedup claims. The original preparation block was captured from the starting main
and executed twice against the same pristine LF checkout and retained Docker layers.
The final shared owner used that same checkout, compiler, flags and layers; only its
build record was removed before the cold measurement.

| Path | Wall seconds | Captured terminal bytes | Build disposition |
| --- | ---: | ---: | --- |
| Original preparation, cold | 55.042 | 4,378 | Image build + `make clean` + full Fortran compile |
| Original preparation, unchanged warm | 62.720 | 4,334 | Image build + `make clean` + full Fortran compile again |
| Shared preparation, cold | 50.313 | 29 | REBUILT; 45 compiler/link commands |
| Shared preparation, unchanged warm | 1.391 | 36 | VERIFIED_REUSE; zero image builds, cleans or compiler commands |
| Cargo `test --lib --no-run`, isolated cold target | 151.704 | 6,179 | Compiled |
| Same unchanged Cargo target | 0.544 | 1,186 | Reused compilation |

The shared owner's complete 27,763-byte compilation/toolchain transcript remains
available on both local paths. The original preparation printed only the last five
Fortran command lines. Both measured original paths invoked Docker image build and
`make clean` once, followed by a full compile. Original direct-driver work rebuilt
the vertical drivers (two compiler calls), interpolation (one, then reused within
its eight-case invocation), and W production (two); the gate deleted the latter
two build directories before every run. The final audit counts those three driver
build groups explicitly.

An earlier attempt to run the complete original gate on Windows stopped at its
ELF executable-permission check after successful compilation (100.648 seconds,
7,202 terminal bytes). A later full local gate reached its fresh vertical drivers
but could not proceed without native `nm`. These are platform limitations, not
passing full-gate evidence; the complete Linux CI gates below are authoritative.

The isolated Cargo target contained 1,550,467,172 uncompressed bytes before
dependency-only filtering. This is not the GitHub archive size or a cache-download
measurement. The successful pre-change Linux technical job
[37828728043](https://github.com/Grodahn/flexpart-gpu/actions/runs/37828728043)
took 270 seconds in its full gate step and 50 seconds in its initial library-test
step; its complete timestamped job transcript contains 320,634 bytes. Its source
was the starting main above. Cross-machine timings are not a controlled speedup
comparison.

The opt-in technical workflow `cache_audit` dispatch runs
`scripts/audit_build_cache.py`: complete cold-compilation, unchanged warm and
forced-clean technical gates on the same Linux runner. Each run retains full
logs, outputs, build metadata and provenance under `target/build-cache-audit/`.
The audit requires the expected rebuild/reuse disposition, identical cold/warm
cache identities, identical executable hashes, exact authoritative scientific
payload hashes across all three runs, and every existing gate's fresh checks.
No previous PASS record is consumed. The normal PR jobs run the original gate
once. [Audit run 37879173210](https://github.com/Grodahn/flexpart-gpu/actions/runs/37879173210)
passed at `1f6d4f7a4eca6d5648f0fe23dc49858c534e2dc0` on Ubuntu 22.04,
Docker 28.0.4, Python 3.10.12 and Vulkan llvmpipe (LLVM 15.0.7).

| Complete technical gate | Wall seconds | Terminal bytes | Parent image builds / cleans / compiler commands | Direct rebuilds / verified hits |
| --- | ---: | ---: | --- | --- |
| Cold compilation miss | 715.734 | 45,895 | 1 / 1 / 45 | 3 / 7 |
| Unchanged warm | 12.177 | 29,974 | 0 / 0 / 0 | 0 / 10 |
| Forced clean | 204.591 | 29,946 | 1 / 1 / 45 | 3 / 7 |

The cold and warm cache key is
`dfd7ae08d72c77d64ae66313a95271347cc4dedfcb79f31192d08ce77224774e`.
All three executable hashes are
`4b79cdf8dbcd1d7ac5362e27cc00dac1aa7717b88f6d8cbe0b79580d41e242cf`.
All six authoritative scientific payload hashes match exactly across all three
runs. Every complete gate produced fresh comparisons, output/provenance records,
software-adapter execution and `TECHNICAL_PASS` with `scientific_verdict:
NOT_EVALUATED`. The final reports and manifests record both pinned checkouts clean;
report generation times differ for each run. No scientific parity is claimed.

Full compiler/toolchain logs retained 157,369 bytes for cold/warm and 156,186 for
forced clean. These image/compile counts cover the pinned FLEXPART parent and its
three direct-driver build groups; separate flex_extract preparation still executes
unchanged. Cold timing includes initial image provisioning and Rust compilation;
forced-clean timing includes a new image/Fortran build with already compiled Rust.
The 715.734-to-12.177 comparison demonstrates same-runner reuse, not a general
hosted-job speedup. Machine-readable results and exact payload identities are in
[build-cache-measurements.json](build-cache-measurements.json); complete retained
artifacts are `ci-technical-gate-37879173210` (30-day CI retention).

## Measured GitHub Cargo restoration

Linux measurements at implementation commit `1f6d4f7a4eca6d5648f0fe23dc49858c534e2dc0`,
Ubuntu 22.04 runner image `20261004.315.1`, Rust 1.99.0, x86_64, Vulkan/Lavapipe,
default Cargo features. These runs are on separate hosted runners, so timings
include runner variation and device execution, not only compilation.

| Software-WGSL step | Missing cache [37879170680](https://github.com/Grodahn/flexpart-gpu/actions/runs/37879170680) | Verified restore [37879526290](https://github.com/Grodahn/flexpart-gpu/actions/runs/37879526290) |
| --- | ---: | ---: |
| Restore | 1 s (miss) | 6 s |
| Checksum verification | <1 s (miss) | 2 s |
| Compile/run preflight and actual WGSL arithmetic | 40 s | 12 s |
| Compile/run required advection smoke | 39 s | 12 s |
| Compile/run canonical forward/backward production advection | 35 s | 27 s |
| Record dependency checksums | 3 s | 1 s |
| Post-job cache publication | 46 s | <1 s (already current) |

The archive is 303,091,708 compressed bytes; its 1,458 retained compilation files
contain 1,357,004,803 uncompressed bytes (the archive also contains registry/git
inputs and checksum metadata). The restore printed `Cargo cache: VERIFIED_REUSE`.
Their complete timestamped terminal transcripts contain 224,901 and 204,575 bytes,
respectively. Both runs passed every existing software-WGSL check with fresh current-revision
reports. The measured recurring restore/verification overhead is smaller than the
63-second reduction across the three named compilation/device steps. First save
cost and runner variance prevent a general whole-job speedup guarantee.

The technical job also restored a separately scoped, verified cache:

| Technical step | Missing cache [37879175284](https://github.com/Grodahn/flexpart-gpu/actions/runs/37879175284) | Verified restore [37880604387](https://github.com/Grodahn/flexpart-gpu/actions/runs/37880604387) |
| --- | ---: | ---: |
| Restore | 1 s (miss) | 7 s |
| Checksum verification | <1 s (miss) | 3 s |
| Compile/run complete required library suite | 70 s | 30 s |
| Complete fresh technical gate | 733 s | 257 s |

The trusted audit dispatch seeded this cache (7-second publication);
PR events themselves never save. The archive is 405,282,571 compressed bytes;
2,388 retained compilation files contain 1,763,892,661 uncompressed bytes.
Both jobs ran the complete required library tests, fresh oracle routines and
software adapter checks. Complete timestamped terminal transcripts contain
256,648 and 228,728 bytes, respectively. The 40-second library-step reduction
exceeds the measured 10-second recurring restore/verification overhead. The
different full-gate timings also include uncached Docker provisioning/network
variation and must not be attributed solely to Cargo caching.

## Proof map

| Requirement | Implementation/check | Retained evidence |
| --- | --- | --- |
| Complete identity, invalidation and atomic publication | Shared #92 owner; source/pin/image/flags/toolchain and each missing/corrupt artifact regression | `test_agent_validation.py` and `test_oracle_build_cache.py` logs |
| Cold/warm/forced-clean equivalence and pristine checkout | Three complete technical gates; exact authoritative payload and executable hashes; all original comparisons | `target/build-cache-audit/{cold,warm,clean}/`, logs and `measurements.json` |
| Rust cache trust, integrity and actual fresh GPU execution | Read-only PR restore policy, dependency-only archive, checksum corruption regressions; full existing jobs after restore | Both workflows' run logs and current-revision GPU artifacts |
| Missing adapter, skipped/incomplete/stale execution fails closed | Existing #175 execution/coverage regressions and unchanged evidence validators; gate clears prior verdicts before prerequisites | Coverage regressions, library log and required software/technical jobs |
| Scientific verdicts, schemas and tolerances unchanged | Scoped diff, complete consumer gates and exact cold/warm/clean scientific payload equality | Fixture comparisons and all gate reports |

## Regression checks

Run `python scripts/test_agent_validation.py`,
`python scripts/test_oracle_build_cache.py`,
`python scripts/test_cargo_cache_integrity.py`,
`python scripts/test_check_ci_test_coverage.py`, and
`python scripts/check_agent_navigation.py` before the required consumer gates.
The #175 inventory updates only its exact gate/header/build-adapter hashes for
this change; its 33 required invocations and 31 driver/configuration cells remain
unchanged; #112 adds its required production-advection step (34 total
source invocations after rebase). No equations, shaders, pins, compiler flags, tolerances or scientific
comparison algorithms change.
