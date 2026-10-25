# Precipitation member/interval prerequisite for #119

Status: **scientific prerequisite unresolved; WGSL implementation stopped**.
Owner: [#119](https://github.com/Grodahn/flexpart-gpu/issues/119).
Prerequisite: [#202](https://github.com/Grodahn/flexpart-gpu/issues/202).
Inspected from main revision `79442a3` on 2026-10-25.
Provenance marker: **GPT6.1 Sol**.

The pinned source establishes the precipitation memory-member arithmetic. It
does not establish which physical precipitation intervals the existing #90
rates must represent to populate those members. #119 explicitly requires that
mapping before implementation. No API, shader, schema extension or production
consumer is added on an assumed mapping.

## Authority and inspected surfaces

Normative FLEXPART 11.1 revision:
`c70586c2b7f5258850705325881c61f557ea9bd8`, `eta=no`, `numpf=1`.
The unchanged source was retrieved from the upstream repository named by the
[reference manifest](../../reference/flexpart-11.1.json).

Read alongside [GPU_CONTRACT.md](../GPU_CONTRACT.md), the
[canonical schema](../meteorology-contract.md),
[interpolation contract, section 5](../interpolation-contract.md#5-2-d-layer--accumulated-field-sampling-interpol_rain),
[accumulation contract](../accumulation-contract.md),
[instantaneous-time contract](../temporal-interpolation.md), and
[#76 composition handoff](../gpu-meteorology-composition.md).
The existing [direct driver](../../scripts/interpolation/direct_interpolation_oracle.f90)
and [link/run harness](../../scripts/interpolation/direct_oracle.sh) are the
original-routine execution boundary.

## Exact pinned source crosswalk

Line numbers refer to unchanged files under the pinned upstream `src/`.

| Source | Established behavior | Limit of the evidence |
| --- | --- | --- |
| `par_mod.f90:190-191` | `numpf=1`, `numwfmem=2`. | Member counts do not define physical averaging intervals. |
| `getfields_mod.f90:243-287` | Loads file `wfname(indj)` into `memind(1)` and the next file into `memind(2)`; assigns their `wftime` values to `memtime(1:2)`. | File/memory timestamps do not identify precipitation accumulation windows. |
| `getfields_mod.f90:159-203` | Keeps a pair while the signed request is inside it; otherwise rotates `memind` and loads the next member. | Forward admission is left-inclusive/right-exclusive while a next field exists. Direct-routine endpoint probes do not prove complete caller-lifecycle coverage. |
| `windfields_mod.f90:425-427,538-559` | Reads `stepRange`, counts precipitation fields and rejects additional fields for `numpf=1`. | This selects precipitation submembers; it does not reconstruct their physical averaging windows. |
| `windfields_mod.f90:2063-2066,2335-2348` | `ipf=istep+1`; copies LSP/CP into `lsprec/convprec(:,:,1,ipf,n)`; clips negative values in this reader. | There is no accumulated-observation differencing here. #119 must not adopt this clipping as a missing-data policy. |
| `windfields_mod.f90:157-158,3440-3444` | Declares stored precipitation as mm/h; the GFS path converts mm/s to mm/h by multiplying by 3600. | Unit conversion does not establish a represented interval. The ECMWF path consumes preprocessed values. |
| `wetdepo_mod.f90:290-327` | Uses the requested `itime`, prepares spatial indices and calls `interpol_rain(itime,...)`. | No precipitation-specific timestamp shift is introduced here. |
| `interpol_mod.f90:1302-1316` | Computes differences from `itime` and memory times; sets `dtt=dt/3`; selects `mp=[1,2]`, `ip=[1,1]`, `dtp1=dt1`, `dtp2=dt2`. | No physical precipitation interval metadata is consumed. |
| `interpol_mod.f90:1344-1354` | Bilinearly samples submember 1 from each memory through `memind(mp(m))`. | This spatial operation is already owned by #87. |
| `interpol_mod.f90:1549-1550` | Divides the weighted precipitation sum by `dtt`. | This proves arithmetic, not the provenance of either input rate. |

Relevant direct edges:

```text
timemanager -> getfields -> readwind_ecmwf / readwind_gfs
timemanager -> wetdepo -> get_wetscav -> interpol_rain
```

Ingestion and sampling are sibling paths. An accumulated amount never enters
`interpol_rain` as such; its precipitation arrays already contain rates.

## Resolved arithmetic and unresolved coverage

For distinct increasing memory times `t0 < t1`, spatially sampled rates `r0`,
`r1` in mm/h and integer-second request `t`, the original routine evaluates:

```text
dt1 = real(t - t0)
dt2 = real(t1 - t)
dt  = real(t1 - t0)
dtt = dt / 3
output_mm_per_hour = (r0 * dt2 + r1 * dt1) / dtt
```

This is three times the linear blend: `3*r0` at `t0`, `3*r1` at `t1`.
The factor 3 is frozen source behavior. It is neither an amount-to-rate
conversion nor proof that an input represents `dt/3` seconds. Output remains mm/h.

The raw routine has no coverage guard and can extrapolate. Its duplicate-time
branch (`interpol_mod.f90:1305-1308`) sets `dt=1`, `dt2=1` when both `dt`
and `dt1` are zero, before forming `dtt`. A future canonical API must explicitly
admit or reject this state. Backward ordering and complete first/last caller
coverage also need explicit admission rules; the ascending fixture does not
establish them.

The effective sampling timestamp is exactly `itime`. What remains unknown is
the relationship of each **prepared rate** to its memory timestamp and physical
interval. A finite production coverage window for #90-derived inputs therefore
cannot yet be declared.

## Why the existing golden does not prove the interval mapping

[#71's fixture](../../fixtures/interpolation/contract-v1.json), case
`rain-layer-fields`, supplies normalized rate grids at memory times `[0,3600]`.
The rain driver scatters them into submember 1 of memory slots 1 and 2. Its
input grammar has no accumulation origin, physical interval endpoints or
preprocessing identity.

[#75's fixture](../../fixtures/accumulation/contract-v1.json), case
`rain-layer-fields-bilinear-rates`, independently constructs:

| Row | Observation valid time | Reset origin | #75/#90 represented interval | Assigned #71 memory time |
| --- | --- | --- | --- | --- |
| 0 | 1800 | 0 | `[0,1800]` | 0 |
| 1 | 3600 | 0 | `[1800,3600]` | 3600 |

LSP source totals `0..5.5` and `50..61` kg/m2 produce interval amounts
`0..5.5` and `50..55.5` kg/m2. Dividing by 1800 s and converting to mm/h
produces `0..11` and `100..111`. CP totals 0.25/1.0 kg/m2 produce rates
0.5/1.5 mm/h. These are valid synthetic interval calculations. The first
source validity time is 1800; its assigned FLEXPART memory time is 0.

At `(xt,yt)=(1.25,0.5)`, those grids yield LSP 3.25/103.25 mm/h and
CP 0.5/1.5 mm/h spatially. At `itime=1800`, the original routine returns
**159.75 mm/h LSP and 3.0 mm/h CP**. Matching these numbers proves their
use in the frozen arithmetic. It cannot independently prove the assignment
in the table's final column.

The accumulation fixture describes `dt/3=1200 s` as a lead/offset interval,
but its actual derived intervals are 1800 s. That description is not an original
preprocessing trace. Neither it nor an assumed reset cadence authorizes a
scientific timestamp mapping.

These fixtures supply no original preprocessing source/revision and prepared
source example with an interval audit proving that assignment. The pinned
reader consumes prepared values without recovering this history. An interval
mean cannot be assumed to equal a preprocessor-reconstructed point rate merely
because both use mm/h.

## Decision and #114 handoff

Stop #119's implementation at this prerequisite, as explicitly required by its
issue and implementation request. [#202](https://github.com/Grodahn/flexpart-gpu/issues/202)
owns the finite original-preparation proof and compatibility decision.
Do not invent a timestamp shift, treat interval ends as instantaneous samples,
scale interval amounts by three, or use #89 as a precipitation fallback.

The existing handoff remains #90 interval products -> #87 spatial sampling
through #76. It preserves amount/SI-rate/mm-h-rate distinctions and interval
identity. It is **not final time-sampled wet-deposition forcing**. There is no
validated #119 output resource for #114 yet.

After #202 resolves the representation, #119 must still supply:

- a typed validated member/interval/time/provenance binding in explicit mm/h;
- #90 -> #87 -> #119 in one caller-owned encoder with resident intermediates;
- source/grid/context identities and resource-lifetime validation;
- actual WGSL execution with adapter, candidate, shader/input and oracle hashes;
- original-routine comparisons at absolute `1e-6` / relative `1e-5`;
- first/interior/last and exact-boundary coverage, nontrivial fields, zero
  precipitation and the complete negative/fail-closed matrix.

Missing members, invalid coverage, source/interval/unit/context mismatches,
nonfinite data, unsupported resets/preparation and `numpf=3` must remain
explicit failures. This documentation-only stop claims no new #119 negative
tests or GPU proof. #114 adoption, cloud science and complete wet-deposition
parity remain separately owned.

## Fresh original-routine probes

On 2026-10-25, built the pinned upstream objects from a `git archive HEAD:src`
scratch copy using `make -f makefile_gfortran FC=gfortran eta=no arch=x86-64 -j4`.
The authoritative LF source checkout was mounted read-only and verified at the
pin with empty `git status --porcelain` before and after execution. The first
Windows CRLF checkout failed the Linux pristine check and was not used as the
oracle source. The driver linked the scratch objects through the unchanged
`direct_oracle.sh`; its required original-routine symbol checks passed.

Image: `sha256:324bb63d7311b13d9700906f05c98babaa69967c1dbfed124abb135855f80d84`.
Compiler: `GNU Fortran (Ubuntu 11.4.0-1ubuntu1~22.04.3) 11.4.0`.
The driver uses its existing `-O0 -fopenmp -mcmodel=large` recipe; upstream
objects use the makefile's `-O3 -UETA -march=x86-64` profile. Execution uses the
manifest's single-thread environment. All non-query input lines are exactly
the frozen `rain-layer-fields` input. Replace its final query count/row with
five `(1.25,0.5,itime,1)` rows for the times below to reproduce the probe input.

| `itime` (s) | `dt1` (s) | `dt2` (s) | `dtt` (s) | Original LSP (mm/h) | Original CP (mm/h) |
| --- | --- | --- | --- | --- | --- |
| 0 | 0 | 3600 | 1200 | 9.75 | 1.5 |
| 900 | 900 | 2700 | 1200 | 84.75 | 2.25 |
| 1800 | 1800 | 1800 | 1200 | 159.75 | 3.0 |
| 2700 | 2700 | 900 | 1200 | 234.75 | 3.75 |
| 3600 | 3600 | 0 | 1200 | 309.75 | 4.5 |

The midpoint LSP/CP values match the existing #71 golden exactly (maximum
absolute difference 0). Other rows are direct oracle observations, not a GPU
candidate comparison. The existing absolute `1e-6` / relative `1e-5` policy is
unchanged. No paired GPU verdict or physical-interval coverage verdict is issued.

Retained local artifacts are under `target/issue-119/` in the original project
checkout: `rain-input.txt`, `oracle-run/rain-output.txt`, `rain-decoded.json`
and `source-trace-report.json` inside `oracle-run/`, plus the complete build,
link, compiler, symbol and checkout-cleanliness records. These ignored local
artifacts are not CI artifacts or a committed reusable fixture pack.

| Artifact | SHA-256 |
| --- | --- |
| Probe input | `8791d0e522a683606c7c414bea69ce8935bc57c3dbfaefca08503acfe03e195f` |
| Raw oracle output | `f5d6b0ffa10dfcc5a22dd7c0545bcdb709d79482064fb85ae86916888dab30e1` |
| Linked oracle executable | `db3c5d345c30dc4f81bb1e3bec2098709fcfbf71bb0829168fbc8c24b2b84880` |
| Sorted linked-object names | `388d1f824306df30fc74fbedc6464e86602b6a93ba782e9e1dad20ffacc3327c` |
| `interpol_mod.f90` | `cc7ab1a55267971ca540d88d3992c613897e0a219da5b66fe2358b5eaeb9bf63` |
| `windfields_mod.f90` | `440ec62b9a7b8f4534e913b2af2a41aabdf44072b3fe77edbc213866e698e7d1` |
| Direct driver | `5749ae531bd593796801f4edf0abdc14db35f8eeceefa2b6d39cfc1ea2de8e69` |
