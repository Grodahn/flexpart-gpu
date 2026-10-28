# Genuine flagged zero-velocity transport-top oracle (#213)

This additive original-science fixture supplies #203 cases C03, C05-C09 and C11
for #214. It changes no production Rust/WGSL, trajectory tolerance, particle ABI,
termination implementation or nonzero settling physics. Provenance marker: **GPT6.1 Sol**.

## Original authority and finite semantics

The unchanged reference is FLEXPART 11.1
`c70586c2b7f5258850705325881c61f557ea9bd8`. The existing
[#180 decision](transport-top-180.md) and [#203 contract](settling-stop-203.md)
remain normative. The source hashes below come from both audited scratch archives;
the full inventory is in the [decoded fixture](../../fixtures/transport/top-boundary-v2/report.json).

| Original file | SHA-256 |
| --- | --- |
| `src/advance_mod.f90` | `7c11a52ad5e0a4b4de36282d72fd4cbebe47e110a30c3609fda3a3ac266d2d68` |
| `src/timemanager_mod.f90` | `93c35cd77de162046c6099e3e3fa4230a65d0f2a5e29bdffb4f108a109f2bd34` |
| `src/com_mod.f90` | `a25dce18fc3d4f3bbbf1b8c64e9b2629fb4e170540d350e555118ab9dd00bab3` |
| `src/readoptions_mod.f90` | `2d28cacdcc65bf22f5c3ec9e4934d07611e3039f5e051ffb2e0b0d7573d3daff` |
| `src/particle_mod.f90` | `c44f670e0b42e2065ecda9e3747b3818f12dd4957cfdaaa777a6570e529aa899` |
| `src/interpol_mod.f90` | `cc7ab1a55267971ca540d88d3992c613897e0a219da5b66fe2358b5eaeb9bf63` |

| Pinned source lines | Confirmed semantics |
| --- | --- |
| `com_mod.f90:175`; `readoptions_mod.f90:2574-2585` | Run-global `lsettling` defaults true; release inventory can disable it. It is independent of instantaneous velocity. |
| `com_mod.f90:219-229,280-288`; `advance_mod.f90:398-425` | Density and carrier properties are species arrays. With domain fill off, select the first release species with mass above `eps3`; only density>0 calls `get_settling`. The sole carrier here has density -1 and release mass 1, so the selected species is unambiguously index 1 and its velocity stays exactly zero. |
| `advance_mod.f90:141-151,690,708-745` | Entry resets `nstop=false` and enabled settling to zero; correction resets/rechecks settling. The wrapper records zero after predictor movement, before/after advanced sampling, after correction and on return. |
| `advance_mod.f90:279-334` | Only the horizontal-exit stop returns immediately. Top action precedes correction; correction gates do not test `nstop`. |
| `advance_mod.f90:664-779` | Correction samples signed advanced time at the stored predictor, including its retained height after a stop. Half the wind difference is applied to that predictor before a second top action. |
| `advance_mod.f90:889-895` | Inclusive `z>=height(nz)`. Backward plus enabled flag sets `nstop`, retaining height. Otherwise ordinary f32 placement occurs. A later predicate never clears a stop; no forward stop rule exists. |
| `timemanager_mod.f90:516-541,562-564`; `particle_mod.f90:357-381` | Caller excludes a stopped particle, then terminates it separately before deposition/decay. Entry resets mean stickiness belongs to one advance call. The direct harness does not claim to execute caller termination. |
| `timemanager_mod.f90:217-218` | Fatal meteorology is an error stop, distinct from a successful scientific particle stop. |

The gas branch intentionally **does not call** positive-density `get_settling`.
The genuine public routine executes both carrier-selection/velocity branches;
observed velocities are `+0.0`, bits `00000000`, while observed `lsettling` is true.
This evidence establishes only the selected single-species gas route, above PBL,
in the #180 regional METRE envelope. It does not validate nonzero settling,
arbitrary species, PBL substeps, ETA, nested/global grids or restart.

## Executed matrix and observation boundaries

The [sibling driver](../../scripts/transport/flagged_top_boundary_oracle.f90)
preserves all scientific inputs and sequencing of frozen #180 scenarios 1-8.
Only the carrier density (-1) and enabled flag differ. Scenario 9 remains the
separate nonzero v1 experiment. There are exactly **64 public advance rows**:
8 scenarios x 2 directions x 2 geometries x 2 epochs. The unchanged eight GRID
and 24 private PUSH records are auxiliary controls, not additional public rows.
Each public row has its own CARRIER record and the following actual observations:

| Observation | Point observed in unchanged original execution |
| --- | --- |
| `INPUT` | Driver state immediately before public advance. |
| `CURRENT_IN`, `CURRENT_OUT` | Before/after genuine `interpol_wind`; entry reset has run. |
| `PREDICTOR_RAW` | Immediately after genuine `update_z_sp` in predictor, before horizontal update/top action. |
| `PLACE_PREDICTOR` | Only if original calls ordinary `set_z_sp` at the first action; observed after that original call. |
| `ADVANCED_IN`, `ADVANCED_OUT` | Before/after genuine `interpol_wind_short`; stored predictor already has first top action. This proves correction executes after a predictor stop. |
| `CORRECTOR_RAW` | Immediately after genuine corrector `update_z_sp`, before horizontal correction/second top action. |
| `PLACE_CORRECTOR` | Only if original calls ordinary `set_z_sp` at the second action. |
| `FINAL` | Returned original particle state. |

The [observers](../../scripts/transport/flagged_top_observer.f90) forward every
argument unchanged through GNU `--wrap`/`__real_` aliases. They only read original
state and write observation records. No authoritative routine source is edited,
and the pristine checkout stays unchanged. As in #180, upstream make stamps its
unlinked normal `FLEXPART.f90` main; source-hash inventories exclude only that
main. Symbol export
for the inherited private probes preserves `.text` and reproduces the whole
linked original object. The final link/disassembly proves public advance comes
from the original object, sampler calls pass through the observers and every
wrapper calls the genuine original routine. Original movement/placement
relocations establish helper edges that `-O3` hoists or keeps private; the audit
does not invent direct public call instructions for those edges.

Each independent build also links and executes an **unwrapped control** from the
same pristine objects and driver. Its GRID/PUSH/CARRIER/ADVANCE output matches
exactly after removing OBS records. This falsifies observer effects on the
scientific result. All input/output x/y/z, velocity and sampled W observations
retain raw f64/f32 hexadecimal bits plus round-trippable decimal text.

`predictor_stop` is classified from directly observed post-action `ADVANCED_IN`
state; `corrector_stop` is the inclusive predicate on observed `CORRECTOR_RAW`
under the source-verified rule. A repeated write of true is not independently
observable as a changed bit. Raw intermediate heights are observed, never
reconstructed from the final height. Placement has a separately observed call.
Both action transitions are audited against the exact source predicates.

## Per-case returned results

The table covers both epochs and directions. Geometry 0 and geometry 1 return
identical values for each row below (all 64 combinations are retained separately
in raw/decoded evidence). Epoch is the original signed multiple of 1200 seconds;
step is signed 600 seconds. P/C indicate predictor/corrector stop predicates.

| Direction | Epoch | Scenario | P stop | C stop | Returned nstop | Returned z (m AGL) |
| --- | --- | --- | --- | --- | --- | --- |
| -1 | 0 | 1 | False | False | False | 8521.84375 |
| -1 | 0 | 2 | True | True | True | 9521.84375 |
| -1 | 0 | 3 | True | True | True | 9522.84375 |
| -1 | 0 | 4 | True | False | True | 9520.84375 |
| -1 | 0 | 5 | False | True | True | 9526.84375 |
| -1 | 0 | 6 | True | True | True | 9526.84375 |
| -1 | 0 | 7 | True | True | True | 9532.843750476837 |
| -1 | 0 | 8 | False | False | False | 8532.04375052452 |
| -1 | 1 | 1 | False | False | False | 8521.84375 |
| -1 | 1 | 2 | True | True | True | 9521.84375 |
| -1 | 1 | 3 | True | True | True | 9522.84375 |
| -1 | 1 | 4 | True | False | True | 9520.843748569489 |
| -1 | 1 | 5 | False | True | True | 9526.84375 |
| -1 | 1 | 6 | True | True | True | 9526.84375 |
| -1 | 1 | 7 | True | True | True | 9532.843749523163 |
| -1 | 1 | 8 | False | False | False | 8532.043750047684 |
| 1 | 0 | 1 | False | False | False | 8521.84375 |
| 1 | 0 | 2 | False | False | False | 9521.8427734375 |
| 1 | 0 | 3 | False | False | False | 9521.8427734375 |
| 1 | 0 | 4 | False | False | False | 9515.8427734375 |
| 1 | 0 | 5 | False | False | False | 9521.8427734375 |
| 1 | 0 | 6 | False | False | False | 9521.8427734375 |
| 1 | 0 | 7 | False | False | False | 9521.8427734375 |
| 1 | 0 | 8 | False | False | False | 8532.04375052452 |
| 1 | 1 | 1 | False | False | False | 8521.84375 |
| 1 | 1 | 2 | False | False | False | 9521.8427734375 |
| 1 | 1 | 3 | False | False | False | 9521.8427734375 |
| 1 | 1 | 4 | False | False | False | 9515.842773914337 |
| 1 | 1 | 5 | False | False | False | 9521.8427734375 |
| 1 | 1 | 6 | False | False | False | 9521.8427734375 |
| 1 | 1 | 7 | False | False | False | 9521.8427734375 |
| 1 | 1 | 8 | False | False | False | 8532.043750047684 |

Scenario 1 is ordinary non-crossing, 2 exactly at top, 3 above, 4 predictor-stop
then inward correction, 5 corrector-only encounter, 6 predictor encounter with
unchanged wind, 7 repeated outward encounters, and 8 independent x/time wind
variation below the top. In backward scenario 6 the retained height is still
above the top at the second action: it is not falsely labeled predictor-only.
Scenario 4 supplies the first-action-only predicate encounter and sticky inward
return. Forward scenario 6 has predictor-only ordinary placement. These are the
original frozen scenarios, with no manufactured forcing or new tolerance.

## Reproduction and audits

Use a pristine LF checkout of the pin and the existing immutable Fortran image.
The launcher independently archives original sources into a new scratch tree on
each call. Original routines use the pinned `FC=gfortran eta=no arch=x86-64 -j4`
make recipe (-O3, METRE); driver/observers use -O0 -fopenmp -mcmodel=large.
All OMP settings are the existing single-thread profile. Use new output paths:

```text
python scripts/transport/run_flagged_top_boundary_oracle.py --checkout <pristine-pin> --output-dir target/flagged-first
python scripts/transport/run_flagged_top_boundary_oracle.py --checkout <pristine-pin> --output-dir target/flagged-second
python scripts/transport/compare_top_boundary_builds.py --first target/flagged-first --second target/flagged-second --output target/flagged-pair.json
python scripts/transport/verify_flagged_top_boundary_oracle.py --fixture fixtures/transport/top-boundary-v2
python scripts/transport/check_settling_stop_contract.py --checkout <pristine-pin>
python scripts/transport/test_top_boundary_oracle.py
python scripts/transport/test_check_settling_stop_contract.py
python scripts/check_agent_navigation.py
```

The frozen [manifest](../../fixtures/transport/top-boundary-v2/manifest.json)
contains every fixture digest, raw/decoded identity and all three historical v1
file hashes. The [paired proof](../../fixtures/transport/top-boundary-v2/reproducibility.json)
contains both independent scratch identities, original/driver/object/executable,
compiler and immutable image identities and full artifact hash inventories.
Full sources, binaries, disassemblies and logs remain under each run's `build/`;
CI retains both complete run directories. The compact committed fixture retains
raw rows, decoded rows/verdicts, input scenario manifest and both inventories,
as in #180. `--record` only captures audited execution for inspection, never a
regression PASS; the inspected baseline is then checked by both actual run audits
and the exact independent-build comparison before acceptance.

The v1 files are byte-identical. Its verifier identity remains historical; the
paired comparator now dispatches v2 in addition to its unchanged v1 path. No v1
expected results or historical provenance hashes are refreshed. The #203 matrix
schema remains v1 because the selector shape is unchanged; `FLAGGED_ADVANCE_V2` is
a named table resolving only to v2 `science.cases` after the full fixture audit.

## Verified capture identities and results

| Evidence | Identity/result |
| --- | --- |
| Independent build 1 | `/tmp/flexpart-top-research.gAnMnv6Q/build/upstream`; report SHA-256 `2d7cace01a99192baf4d40df9297ac5e659d3555634bafea7c0e621f7d53715a` |
| Independent build 2 | `/tmp/flexpart-top-research.k9pdjZYM/build/upstream`; report SHA-256 `18207d2d40891e8bf5e36df3605edc6d480229211e58469cd48a88f269beabda` |
| Raw observed output SHA-256 | `427809dfaaea7d57cb8719fed97b3fa2b13c31106b822e8756f4c4e89444369b` |
| Decoded baseline report SHA-256 | `d7607c0fc4242d96fe6d57ffc889f163661fbe6a474445e4804bdcbbe85b6e9a` |
| Canonical decoded science SHA-256 | `fe58354beee26687feb671442404f4c44bd98f5689270e26f0feeb530b0732fa` |
| Independent comparison | PASS, exact raw bytes and decoded observations, two distinct clean scratch origins. |
| Original carriers/flags | All 64 rows: selected species 1, density -1, `lsettling=true`, settling exactly `+0.0` / `00000000`. |
| Frozen v1 audit | PASS for all three historical files, unchanged bytes and digests. |
| Oracle regression suite | 19 tests PASS, including copied-evidence mutations. |
| #203 auditor regression suite | 22 tests PASS. |
| Navigation | 12 surfaces PASS; 26 checker regression tests PASS. |
| Pinned #203 contract audit | PASS: 25 cases, 23 original source anchors, 22 owner symbols, 310 resolved referenced rows. |

All required issue-owned original-routine gates pass. Cargo suites and GPU builds
were omitted as requested: no production Rust/WGSL changes. The optional existing
#198 device-evidence regression could not execute in this isolated checkout
because its generated device report is absent; the older cached report has ten
mismatched source-file identities and was not reused. This is separate from the
successful original-only gates; no device validation is claimed. Local Python
mutation tests used workspace-local TMP/TEMP because desktop sandbox permissions
prevented use of the default Windows temporary directory.

## Exact #214 handoff

1. Audit the complete v2 fixture and #203 contract before consuming it. Load
   `report.json` -> `science.cases`, select by
   `(geometry, direction, epoch_1200_s, scenario)`; each selector resolves once.
   The row `id` is stable only within this fixture version. `scenarios.json`
   enumerates all 64 selectors and exact expected field identities.
2. Bind the original input, selected species index 1, gas density -1,
   `settling_enabled=true`, `carrier` properties, signed times and shared target
   from GRID. Use the same #180 winds and complete prepared vector route.
3. Consume `stages.PREDICTOR_RAW`, `ADVANCED_IN`, `CORRECTOR_RAW`, `FINAL`,
   raw bits, `predictor_stop`, `corrector_stop`, `nstop` and returned position.
   Prove retained predictor sampling and correction after stop; scenario 4 must
   return stopped after inward correction. Forward cases must never gain a stop.
4. Resolve the existing #203 selectors named `FLAGGED_ADVANCE_V2`; C03, C05-C09
   and C11 now await #214 implementation, not oracle generation. Individual lane
   values also support C12-C14, C17/C18/C20 and the gas part of C22, without
   satisfying their structural/device/driver/producer obligations.
5. Keep #203 fatal-status atomicity, publication, eligibility and ownership rules.
   This fixture supplies original values only. #214 must add its own genuine GPU
   execution and production-path gates; no tolerance is added here. Do not treat
   #180's f64-only private probes as representable GPU cases or infer complete
   GPU/production parity from this evidence.
