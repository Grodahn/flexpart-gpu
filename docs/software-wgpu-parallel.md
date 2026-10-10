# Software WGSL domain orchestration (#177)

Contract: [issue #177](https://github.com/Grodahn/flexpart-gpu/issues/177).
Baseline main: `2c4dc3d39502673bf4bed76e14855a44b9ee2d6c`.
The branch was subsequently rebased onto `217d3c1` after main merged #191.
Its added interior-W research oracle remains required in the untouched technical
workflow. The software baseline did not change; the inventory separately records
the preserved independent-workflow revision and hashes.
Externally supplied provenance marker: **GPT6.1 Sol**.

The [frozen before/after mapping](software-wgpu-before.json) contains each original
step body, exact Cargo arguments and environment, expanded configurations, original
artifact paths, source hashes and post-split owners. Each original mandatory command
and scientific assertion remains verbatim. The #175 auditor reads a normalized view
of actual domain steps; its historical inventory, selection and log semantics remain
unchanged. Run `python scripts/software_wgpu_evidence.py audit` and
`python scripts/check_ci_test_coverage.py` to verify preservation.

| Independent domain | Original obligations |
| --- | --- |
| `gpu-meteorology` | #90 accumulation; #89 temporal; #88 vertical/W; #87 horizontal; #171 resident queries/status; #76 composition; #173 canonical driver preparation and lineage; all original scientific validators and reports |
| `gpu-transport-physics` | Analytical advection; settling; PBL; forward/backward; all four compaction × validation cells; dry/wet prefix and bounds; Hanna/Langevin; deterministic corpus; #112 production advection in production, validation and compaction modes |
| Both domains | Explicit Lavapipe ICD/software/Vulkan settings; unchanged trusted dependency cache restoration/integrity/publication; formatting; real H2D/WGSL/D2H preflight |

Runtime smoke is folded into both domains, requiring each independent runner to
prove its adapter before its scientific tests. No GPU domain depends on another.
#76 consumes earlier meteorology evidence, so its producers stay on that runner.
Device creation remains serialized wherever the original command required it.
The technical oracle gate and manual extended workflow are byte-for-byte unchanged
after newline normalization; their identities are frozen in the mapping.

The pinned Rust cache action includes `github.job` in its automatic key. Renaming
jobs initially caused complete compilation cache misses despite identical explicit
keys and dependencies. Both domains therefore set `shared-key` to the original
additional key followed by `-software-wgpu`. The pinned action's `src/config.ts`
uses shared-key in place of the entire key-plus-job prefix, so a job-name-only
shared-key also misses the original cache. Materializing the complete prefix
preserves #176's actual complete cache key; its action pin, dependencies, integrity checks, archived
directories and PR read-only publication policy remain unchanged. The mapping
auditor permits only this exact namespace preservation and rejects other values.

`software-wgpu` remains the externally visible aggregate check, with both required
domains in `needs` and `if: always()`. No trigger/path filter, matrix or optional
domain can bypass it. GitHub's main branch-protection endpoint reported “Branch
not protected” and the repository ruleset list was empty at implementation; no
protection settings were changed and the original check name is preserved.

Each domain uploads `software-wgpu-<domain>-<run_id>-<run_attempt>`, preserving the
original `target/` layout inside an isolated bundle. `manifest.json` binds candidate
PR head, actual checked-out source (GitHub's PR merge SHA), workflow run/attempt,
original required contract hash, every required Actions step outcome/conclusion,
positive executed-test counts, adapter/backend and all retained file SHA-256s.
The baseline's existing head/merge revision conventions remain unchanged.
Failed domains retain available logs and an incomplete manifest; these cannot pass.
No generated scientific evidence or verdict enters a cache.

The aggregate downloads artifacts from its own run, rejects other attempts and
rechecks actual bytes and positive log counts. It repeats the original inline
scientific Python assertions against the current checkout's source and fixtures,
reuses #175's driver-log checks and #112's evidence validator for all three retained
configurations, and validates accumulated-cell counts/verdicts. It does not trust
a summary `PASS` or artifact filenames alone. Download, parse, adapter, source,
hash, comparison, command, cancellation and skip failures all fail the required job.
Its own compact report is uploaded as `software-wgpu-aggregate-<run_id>-<attempt>`.

Run `python scripts/test_software_wgpu_evidence.py` for focused negative fixtures.
The handoff tests use small synthetic files and separately test genuine Bash exit
37 despite a misleading `PASS` marker. Scientific assertions remain covered by
the existing validators and actual CI artifacts, rather than fabricated parity
fixtures. Runtime-step failure is tested inside its owning domain because runtime
has no separate runner. The existing coverage negative tests still exercise the
original changed Bash commands and their nonzero statuses.

To diagnose failure, first inspect the failed domain's named Actions step and its
retained original log/report path. A seal failure names the missing stage, report
or execution condition. An aggregate failure names the domain, identity, file hash
or repeated scientific assertion. Inspect its `software-wgpu-aggregate.json` and
the domain manifests before rerunning tests. Missing artifacts are failures even
when a cancelled runner cannot upload its logs.

The timing comparison is recorded in [measurements](software-wgpu-measurements.json)
from actual complete GitHub runs. Two GPU runners were chosen to bound repeated setup and
restore overhead; a third runtime runner would repeat setup for a short smoke.
Measurements distinguish workflow elapsed time, critical path, overlapping domain
intervals, setup/cache/test durations, total job runner minutes, terminal bytes and
artifact counts/compressed sizes. Hosted-runner variation and different compilation
cache dispositions limit attribution. This orchestration makes no scientific parity
or hardware-GPU performance claim.

## Observed timing and runner cost

| Complete successful software run | Wall / critical path (s) | Runner minutes | Verified Cargo restores | GPU overlap (s) | Transcript bytes | Artifact count / compressed bytes |
| --- | --- | --- | --- | --- | --- | --- |
| [Historical main](https://github.com/Grodahn/flexpart-gpu/actions/runs/37930806734) | 161 / 158 | 2.63 | 1 | 0 | 160,162 | 1 / 720,323 |
| [Current-environment monolith](https://github.com/Grodahn/flexpart-gpu/actions/runs/38063045486) | 139 / 135 | 2.25 | 1 | 0 | 204,281 | 1 / 720,323 |
| [Initial split, cache misses](https://github.com/Grodahn/flexpart-gpu/actions/runs/38062644702) | 169 / 166 | 4.83 | 0 | 127 | 361,036 | 3 / 738,463 |
| [Rebased split, cache misses](https://github.com/Grodahn/flexpart-gpu/actions/runs/38063706325) | 154 / 151 | 4.47 | 0 | 119 | 360,944 | 3 / 738,738 |
| [Exact original cache keys restored](https://github.com/Grodahn/flexpart-gpu/actions/runs/38063861101) | 147 / 145 | 4.08 | 2 | 102 | 340,321 | 3 / 734,172 |

The restored-cache split's meteorology job took 103 s and transport 135 s; the
aggregate took 7 s. Setup including restores took 39/30 s, cache restore 10/7 s,
integrity verification 3/3 s, preflight compilation/smoke 19/12 s, and scientific
compile/test/assertion steps 40/86 s. The current-environment monolith spent 27 s
in setup, 5 s restoring, 2 s verifying, 11 s in preflight and 93 s in scientific
steps. Retained libtest execution summaries sum to 4.06 s for meteorology and
50.81 s for transport in the split; step durations additionally include compilation.
All 59 meteorology and 74 transport tests, 177 required payload files, two domain
manifests and the aggregate report were independently inspected after download.
Both domain keys exactly match the original full cache key, and their fresh
scientific evidence is independent of the restored compilation bytes.

This sample improved wall time by 14 s versus historical main but regressed by
8 s (5.8%) versus the current-environment monolith. Runner occupancy increased by
1.83 minutes (81.5%) against the latter. **No consistent wall-clock speedup is
demonstrated.** The two-domain layout is already the smallest split that preserves
required concurrent independence; adding a third runtime runner is not justified
by the measured repeated setup/restore cost. No third-runner experiment was run.
The observations are retained, including unsuccessful optimization outcomes;
runner/network/compilation variance prevents a general speedup guarantee.

[Actual failed-domain run](https://github.com/Grodahn/flexpart-gpu/actions/runs/38062368049)
also proves workflow-level failure behavior: transport succeeded, meteorology
failed, and the required aggregate still ran and failed with
`required domain failed/cancelled/skipped`. Sixteen focused handoff/exit/cache
negative tests and the seventeen existing coverage/shell tests pass. Formatting,
Clippy (existing warnings), full Cargo tests and navigation passed locally.
The PR links final required CI against its reviewed head, including the preserved
new main interior-W oracle gate. No issue closure or scientific parity is claimed.
