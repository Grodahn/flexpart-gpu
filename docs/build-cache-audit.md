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
and rebuild normally. Required tests and GPU evidence checks always follow.
PR events can restore scoped caches but cannot save them. Main cannot read
feature/PR-scoped caches; trusted manual audit runs can save their own branch
cache without making it available to protected main.

## Measurements and limitations

Measured on Windows, Docker Desktop 29.8.0, GNU Fortran 11.4.0, with existing
Docker layers and downloaded Cargo dependencies retained. Full logs and JSON
measurements are under `target/issue176-evidence/` in the implementation worktree.
These initial component measurements do not claim a full scientific-gate speedup:

| Path | Wall seconds | Captured terminal bytes | Result |
| --- | ---: | ---: | --- |
| Pre-change gate through unconditional full Fortran build | 100.648 | 7,202 | Build completed; Windows ELF executable-permission check failed |
| Shared oracle preparation, compilation-cache miss | 44.450 | 29 | REBUILT |
| Same unchanged oracle preparation | 1.764 | 36 | VERIFIED_REUSE |
| Cargo `test --lib --no-run`, isolated cold target | 151.704 | 6,179 | Compiled |
| Same unchanged Cargo target | 0.544 | 1,186 | Reused compilation |

The Cargo target contained 1,550,467,172 uncompressed bytes before dependency-only
filtering. This is not the GitHub archive size or a cache-download measurement.
The successful pre-change Linux technical job
[37828728043](https://github.com/Grodahn/flexpart-gpu/actions/runs/37828728043)
took 270 seconds in its full gate step and 50 seconds in its initial library-test
step. Its source was the starting main above. Cross-machine timings are not a
controlled speedup comparison.

The opt-in technical workflow `cache_audit` dispatch runs
`scripts/audit_build_cache.py`: complete cold-compilation, unchanged warm and
forced-clean technical gates on the same Linux runner. Each run retains full
logs, outputs, build metadata and provenance under `target/build-cache-audit/`.
The audit requires the expected rebuild/reuse disposition, identical cold/warm
cache identities, identical executable hashes, exact authoritative scientific
payload hashes across all three runs, and every existing gate's fresh checks.
No previous PASS record is consumed. The normal PR jobs run the original gate
once. Full audit/restore measurements and final CI links are recorded after
verification; #176 remains open until its proof obligations are demonstrated.

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
