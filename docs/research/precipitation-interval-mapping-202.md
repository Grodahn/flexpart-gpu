# Precipitation member/interval mapping for `numpf=1` (#202)

Status: **decided — INCOMPATIBLE**. #90 interval-mean rates cannot directly
populate the members read by pinned FLEXPART 11.1 `interpol_rain`. The missing
operation is tracked by [#207](https://github.com/Grodahn/flexpart-gpu/issues/207);
[#119](https://github.com/Grodahn/flexpart-gpu/issues/119) stays blocked on it.
Inspected from main revision `79442a3` on 2026-10-27.

Machine-readable decision: [precipitation-member-mapping-v1.json](../../fixtures/accumulation/precipitation-member-mapping-v1.json).
Decoded evidence: [precipitation-member-mapping-evidence-v1.json](../../fixtures/accumulation/precipitation-member-mapping-evidence-v1.json).
Regression lock: [tests/precipitation_member_mapping.rs](../../tests/precipitation_member_mapping.rs).

This builds on the consumer-side audit in draft
[PR #205](https://github.com/Grodahn/flexpart-gpu/pull/205)
(`docs/research/precipitation-time-119.md`), which froze the FLEXPART reader,
memory rotation and `interpol_rain` arithmetic and reproduced the #71 golden.
Those findings are reused, not repeated. This note adds the upstream
preparation side, executes it, and issues the verdict #205 deferred.

Validation level: original-source analysis, execution of unmodified pinned
preprocessing, and a direct original-routine oracle. **No GPU execution and no
production-parity claim.**

## References

| Reference | Pin | Role |
| --- | --- | --- |
| FLEXPART 11.1 ([manifest](../../reference/flexpart-11.1.json)) | `c70586c2b7f5258850705325881c61f557ea9bd8`, `eta=no`, `numpf=1` | Reader, memory rotation, `interpol_rain` |
| flex_extract 7.1.2 ([manifest](../../reference/flex-extract.json)) | `e0005c99ac81d12faa45a8ff799debbd592b0dc0` | ECMWF preparation of LSP/CP members |

The flex_extract pin is the repository's existing preprocessing reference.
Its `deacc_fluxes` path is the only producer of precipitation members for the
ECMWF reader that this repository pins.

## Phase 1 — source-to-consumer chain

`raw accumulation → deaccumulation → darain → flux GRIB → output file → AVAILABLE/memtime → interpol_rain`

| Stage | Pinned source | Quantity, units | Interval / time meaning |
| --- | --- | --- | --- |
| MARS flux retrieval | `EcFlexpart.py:312-333`; padding `get_mars_data.py:299-301` | LSP 142 / CP 143 accumulated since forecast base, m | `[base, base+step]`, steps `DTIME..ACCMAXSTEP`; flux days padded ±1 day unless `BASETIME=0` |
| Deaccumulation | `EcFlexpart.py:1123-1143` | `x_k = (A_k − A_{k−1})·1000 / DTIME`, mm/h | Interval mean over `(t_k−Δ, t_k]`, `Δ = DTIME` in integer hours |
| Reconstruction (`RRINT=0`) | `disaggregation.py:106-157`, called at `EcFlexpart.py:1180-1185` | `darain(x_{k−1}, x_k, x_{k+1}, x_{k+2})`, mm/h | Rate at the instant `T = t_k`, support `[T−2Δ, T+2Δ]` |
| Prepared GRIB | `EcFlexpart.py:1201-1211` | One LSP and one CP message, `stepRange=0` | File `flux<T>`, `dataDate/dataTime = T` |
| FLEXPART input file | `EcFlexpart.py:1739-1765` | `flux<T>` concatenated into `<PREFIX><YYMMDDHH of T>` | Same `T` |
| Reader | `windfields_mod.f90:541-548, 2063-2066, 2180, 2335-2348` | `ipf = stepRange+1 = 1`; real4; negative clip; GRIB1 CP factor 1 | `memtime = wftime` from AVAILABLE; GRIB date/time is never read |
| Consumer | `interpol_mod.f90:1302-1316, 1549-1550` (see #205) | `(r0·dt2 + r1·dt1)/dtt`, `dtt = dt/3`, mm/h | Unshifted `itime` |

`darain` clips each interval mean at zero, then forms
`xb·xc/(xa+xc) + xb·xc/(xb+xd)`. A zero denominator falls back to `0.5·xb` or
`0.5·xc`. Its docstring describes the output as a point value used for later
linear interpolation. For a linear ramp it returns the exact instantaneous rate
at `T`, and it preserves a constant series.

## RRINT and numpf

- **RRINT=0 ↔ numpf=1 is proven by the reader/writer pair.** `RRINT=0` writes
  two precipitation messages per file with `stepRange=0`. The `numpf=1` reader
  accepts at most two (`windfields_mod.f90:541-548`). Executed: two messages
  in every file.
- **RRINT=1 is unusable with numpf=1.** It writes six messages per file with
  `stepRange` 0/1/2 (`EcFlexpart.py:1411-1458`). Executed: six per file, which
  makes `readwind_ecmwf` stop with "set numpf to 3".
- **PUREFC=1** stamps `stepRange = step − 2·DTIME` on members
  (`EcFlexpart.py:1204-1205`). The reader turns that into `ipf > 1`. Rejected
  from source; not executed.
- **BASETIME**: the last-step branch (`EcFlexpart.py:1218-1253`) writes the raw
  last interval mean at `T_last`. At `T_last−Δ` it writes
  `darain(reversed(...))`, which is algebraically the `T_last−2Δ` value.
  Executed: the `T_last−Δ` member equals the `T_last−2Δ` member exactly.
  Rejected.

## Phase 2 — `dt/3`

FLEXPART commit `c655930217a164cebfa0c5397636fbfa9e856d60` (2023-07-12,
ticket #295) introduced `dtt = dt/3` together with `numpf=3` sub-members.
Before it, v10.4 returned `y1(1)` with no temporal interpolation, and the
pre-#295 v11 `wetdepo` used the nearer memory. In every `numpf=3` branch
`dtp1 + dtp2 = dtt` (`interpol_mod.f90:1318-1340`). The `numpf=1` branch keeps
`dtp1 + dtp2 = dt = 3·dtt`. The divisor is unchanged on upstream master
`d4e0a868` (2026-10-09).

Upstream compensation: none. `darain` maps a constant sequence to itself, and
the GRIB1 reader does not scale LSP/CP. Executed result: constant 1 mm/h
interval means become 1 mm/h members, and `interpol_rain` returns 3 mm/h at
every admitted time. Across all decoded node rows, the original output is three
times the linear blend of the prepared members, within the #71 policy.

No physical meaning of the factor is established. It is frozen pinned arithmetic
inherited from the `numpf=3` path. #119 must reproduce it exactly. Removal or
compensation is not authorized.

## Phase 3 — fixture association

The #75 case `rain-layer-fields-bilinear-rates` is **INCOMPATIBLE** with
original preprocessing:

- It maps interval means directly to memory times. An interior flex_extract
  member is never an interval mean.
- It assigns `(0,1800]` to memory time 0, the start of that interval. No path
  stamps an interval mean at its start.
- It assigns `(1800,3600]` to 3600, its end. flex_extract stamps an interval
  mean at its end only in the rejected boundary writes: the leading persistence
  value and the BASETIME trailing write.
- `Δ = 1800 s` is not an integer-hour `DTIME`.
- Two intervals cannot supply the four-interval support of any member.

The 159.75 / 3.0 mm/h golden remains a valid regression for the pinned
arithmetic. It is not mapping evidence.

## Phase 4 — executed example

The example is a deterministic OD-style accumulated sequence: 3×3 GRIB1 grid,
forecast bases 2023-11-01/02 at 00 and 12 UTC, steps 3/6/9/12 h, `DTIME=3`.
That gives 16 contiguous intervals with constant, zero, burst, ramp,
alternating and realistic series for LSP and CP (24-bit packing). Unmodified
pinned `deacc_fluxes` was run three ways: `RRINT=0`, `RRINT=0` with
`BASETIME=0`, and `RRINT=1`. Instrumentation only observes `darain` and
`codes_write`.

Results:

- Members exist at `T = 06 UTC day 1 … 18 UTC day 2`, which is
  `T_first = s₀+2Δ` to `T_last = s₀+NΔ−2Δ`.
- Without padding, the first flux file (03 UTC) carries the persistence value,
  which is its own interval mean. The last two files are empty.
- Recorded `darain` results equal the decoded prepared members exactly.
- Example at node (1,1), LSP: neighbouring interval means 0.15 and 0.9 mm/h
  give a member at 06 UTC of 0.21659 mm/h. Most nonzero interior members
  differ from both adjacent interval means.

Genuine `interpol_rain` was built from a `git archive` scratch copy of the
pinned checkout, linked through the unchanged
[`direct_oracle.sh`](../../scripts/interpolation/direct_oracle.sh) and fed the
prepared members. The checkout was verified clean before and after. Six pairs
were probed at `−1 h, 0, +1 h, +1.5 h, +2 h, +3 h, +4 h`:

- the leading boundary;
- the first interior pair;
- two interior pairs that share the member at 75600 s;
- the last interior pair;
- the real-caller duplicate-time state at `T_last`.

Every node row matches `(r0·dt2 + r1·dt1)/dtt` within `1e-6 + 1e-5·|o|`.
At the exact interior boundary (75600 s), both adjacent pairs return
identical values. The raw routine extrapolates outside its pair. At `T_last`
the duplicate branch (`dt=1, dt2=1, dtt=1/3`) returns `3·r(T_last)`.

| Pair (s) | itime | Members r0, r1 (mm/h) | Linear | Original LSP |
| --- | --- | --- | --- | --- |
| 21600, 32400 | 21600 | 0.21659, 1.592027 | 0.21659 | 0.64977 |
| 21600, 32400 | 27000 | 0.21659, 1.592027 | 0.904309 | 2.712926 |
| 140400, 151200 | 151200 | 0.777328, 0.134616 | 0.134616 | 0.403847 |
| 151200, 151200 | 151200 | 0.134616, 0.134616 | 0.134616 | 0.403847 |

Reproduction:

```bash
docker run --rm -v <repo>:/workspace/flexpart-gpu -v <flex_extract clone>:/workspace/flex_extract:ro --entrypoint python3 flex-extract:latest -I /workspace/flexpart-gpu/scripts/precipitation/flex_extract_member_trace.py --flex-extract-git /workspace/flex_extract --out /workspace/flexpart-gpu/target/issue-202/flex-extract
```

```bash
docker run --rm -v <repo>:/workspace/flexpart-gpu -v <clean FLEXPART pin>:/workspace/flexpart:ro --entrypoint bash flexpart-fortran:latest /workspace/flexpart-gpu/scripts/precipitation/member_mapping_oracle.sh /workspace/flexpart-gpu/target/issue-202/flex-extract /workspace/flexpart-gpu/target/issue-202/oracle
```

```bash
python -I scripts/precipitation/decode_member_mapping_evidence.py --trace target/issue-202/flex-extract/flex-extract-trace.json --oracle target/issue-202/oracle --out fixtures/accumulation/precipitation-member-mapping-evidence-v1.json
```

Raw trace, flux GRIB, oracle outputs, build logs and checkout records are kept
locally under `target/issue-202/`. They are not CI artifacts. Their hashes are
in the decision artifact. Images: `flex-extract`
`sha256:717f14df…`, `flexpart-fortran` `sha256:324bb63d…`.
Compiler: gfortran 11.4.0. ecCodes 2.24.2.

Not executed: full `readwind_ecmwf` on a complete FLEXPART input file. The
prepared GRIB keys were decoded with the same ecCodes keys that the reader
requests.

## Phase 5 — handoff

The decision artifact enumerates the following for #119:

- the accepted representation: the `RRINT=0` reconstructed point rate at `T`,
  in mm/h;
- the member-to-support mapping and memory-time semantics;
- required provenance, coverage and boundary policies;
- twelve rejection codes;
- the typed input that #119 must accept once #207 exists.

Coverage, for `N ≥ 5` contiguous Δ intervals from `s₀`:

- members at `T_j = s₀ + jΔ` for `j = 2..N−2`;
- admitted `T_first ≤ itime ≤ T_last`, forward time only;
- pair selection is left-inclusive and right-exclusive;
- `T_last` uses the duplicate-time branch.

## Rejected assumptions

- `RRINT=0` equals `numpf=1` merely because both use one member.
- A member equals an interval mean.
- `dt/3` converts an interval amount or encodes a one-third interval.
- Some upstream step divides by three.
- The fixture's "20-minute lead/offset" reading of `dtt`.
- Interval starts, ends or centres are member timestamps.
- GRIB date/time constrains FLEXPART memory time.

## Out of scope observations

- `RRINT=1` files stamped `T` carry IA3 values at `T−Δ`, `T−2Δ/3` and
  `T−Δ/3`. `numpf=3` `interpol_rain` places sub-members at `memtime`,
  `+dt/3` and `+2dt/3`. Any `numpf=3` work must resolve this one-interval
  offset first.
- The GRIB2 CP path divides by 1000 (`windfields_mod.f90:2140-2147`).
- ERA5 class `EA` skips deaccumulation (`EcFlexpart.py:1136-1138`). This
  decision covers the exercised GRIB1 OD-style path only.
