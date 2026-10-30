# Precipitation point members for `numpf=1` (#207)

Status: GPU reconstruction stage implemented. Since #219 the genuine #202
accumulated example enters it through #90 on the production path (see
[#216 admissions](#genuine-202-production-replay-216219)).
Decision basis: [#202 note](research/precipitation-interval-mapping-202.md) and
[decision artifact](../fixtures/accumulation/precipitation-member-mapping-v1.json).

```text
#90 interval-mean rate grids -> #207 darain -> members at T -> #119 time sampling
```

#207 produces reconstructed members only. It does not sample them in time,
reproduce `interpol_rain`'s `dt/3`, or adopt them in wet deposition (#119, #114).

## Algorithm

Original: flex_extract 7.1.2 `disaggregation.darain`
(`Source/Python/Mods/disaggregation.py:106-157`, commit `e0005c99`, blob
`12ec74a7`), called for LSP and CP with `RRINT=0` by `EcFlexpart.deacc_fluxes`
(`EcFlexpart.py:1180-1185`). For each parameter and horizontal cell, with the
interval means `xa..xd` over `(T-2Δ,T-Δ]`, `(T-Δ,T]`, `(T,T+Δ]`, `(T+Δ,T+2Δ]`:

```text
x    = x < 0 ? 0 : x                 (numpy x[x < 0.] = 0.)
xac  = xa + xc > 0 ? xb*xc/(xa + xc) : 0.5*xb
xbd  = xb + xd > 0 ? xb*xc/(xb + xd) : 0.5*xc
r(T) = xac + xbd
```

The transcription was checked against the pinned source. The original clips
in place on the arrays it shares with the next call; clipping is idempotent, so
results are unchanged. Kernel: [`darain_precipitation_member.wgsl`](../src/shaders/darain_precipitation_member.wgsl).
The oracle is float64; the kernel is `f32` and is compared with the #71 rule
`|c-o| <= 1e-6 + 1e-5|o|`. No new tolerance is introduced.

## Supported envelope and timestamps

Host validation runs before any GPU work. Admitted:

- LSP and CP canonical `AccumulatedSinceReset` surface fields (`kg/m2`,
  non-negative) resolved by #90, in that slot order;
- the same source snapshots, grid, calendar, intervals and `GpuContext`;
- `N >= 5` contiguous intervals of one positive integer-hour length `Δ`;
- preparation `RRINT=0`, `PUREFC=0`, `BASETIME` unset, `numpf=1`, `darain`.

For intervals `(s0 + kΔ, s0 + (k+1)Δ]`, members exist at `T_j = s0 + jΔ`,
`j = 2..N-2`, each with support `[T-2Δ, T+2Δ]`. The leading persistence value
(`s0+Δ`) and the BASETIME trailing writes are never produced. Member slot `m`
uses intervals `m..m+3` and has `T = end(interval m+1)`.

Rejections use the #202 codes through `PrecipitationMemberError::rejection_code`:
`unsupported_preparation` (RRINT=1/IA3, PUREFC=1, BASETIME, numpf≠1, other
methods, interval-total or other unproven conventions), `identity_mismatch`
(field order, source snapshots, grid, calendar, intervals, context),
`insufficient_support`, `gap_or_overlap`, `unequal_or_non_integer_hour_delta`
(also a missing observation inside a run, which #75 turns into a `2Δ`
interval), `non_finite`, `outside_coverage`, `member_spacing_mismatch`.
A declared reset after an uncovered gap, non-increasing observation times and
non-finite accumulations are already rejected by #75/canonical upload. A missing interval never becomes zero.

## API and handoff for #119

Owner: [`src/gpu/precipitation_members.rs`](../src/gpu/precipitation_members.rs).
#90 view: [`CanonicalGpuField::interval_rate_grid` / `encode_interval_rates`](../src/gpu/meteorology/interval_rates.rs).

1. Upload LSP and CP with `CanonicalGpuField::upload` from the same snapshots.
2. `PrecipitationMemberKernels::new(ctx)` once.
3. `prepare_precipitation_members(ctx, &lsp, &cp, PrecipitationMemberPreparation::flex_extract_rrint0())`
   validates and allocates.
4. `prepared.encode(ctx, &kernels, &mut encoder)` resets the status word and
   records #90 (LSP, CP) and `darain` (LSP, CP) into the caller's encoder. The
   #90 rate buffers are bound in place. Nothing is submitted or read back.
5. The returned `EncodedPrecipitationMembers` exposes:
   - `large_scale_mm_per_h`, `convective_mm_per_h`: `STORAGE | COPY_SRC` `f32`,
     `[slot * cell_count + cell]`, x-fastest, mm/h;
   - `status`: one `u32`; bit 1 non-finite rate, bit 2 non-finite member. A
     consumer must reject any nonzero value;
   - `metadata`: representation `flex_extract-7.1.2-rrint0-darain`, unit,
     calendar, `Δ`, `s0`, grid, cell count, source snapshot hashes, #75
     intervals, LSP/CP source identities, adapter, and every member's slot,
     `T`, support bounds and four support intervals. Each source identity also
     carries the #90 `quantization_admissions` (#216/#219): the typed record of
     every `(interval, cell)` projected to `+0.0`. It is omitted when empty and
     is part of `input_sha256`. #119 must treat it as provenance; the signed
     difference it holds is never a precipitation rate.

#119 selects members with `metadata.member_at(T)` and binds planes with
`metadata.plane_byte_range(member)`. It must not reconstruct rates, shift
timestamps, rescale, or add a tolerance. Keep the prepared owner and sources
alive until the last submitted consumer completes. `download` is a validation
boundary only.

`reconstruct_members_from_rates_gpu` runs the same kernel on validated raw
rates for oracle validation (including negative rates #90 never produces). It
returns no typed metadata and is not a production path.

## Evidence

Oracle: [`precipitation-darain-oracle-v1.json`](../fixtures/accumulation/precipitation-darain-oracle-v1.json),
written by [`darain_member_oracle.py`](../scripts/precipitation/darain_member_oracle.py)
in the pinned `flex-extract` image. It reuses the #202 raw trace (its hash must
equal the decision artifact's `raw_trace_sha256`) and executes the unmodified
pinned `darain` loaded from the flex_extract git object store.

- `genuine`: GRIB-decoded accumulations, all 16 `deac_vals` per cell as seen
  before clipping, and all 13 float64 `darain` results per parameter.
- `direct`: zero, constant, alternating wet/dry, isolated burst, negative,
  both zero-denominator branches, magnitude range, minimum `N=5` support and
  the dyadic accumulated sequence used for the production test.

Device test: [`tests/precipitation_members_gpu.rs`](../tests/precipitation_members_gpu.rs);
reports in `target/ci-gate/precipitation-members/`.

| Report | Proves |
| --- | --- |
| `genuine-input-equivalence` | Per series with #216 records: #90 GPU amount, duration and mm/h rate equal `max(deac_vals, 0)` for 18 of 18 series, exact `+0.0` for the 3 admissions; without records those 3 still fail closed |
| `genuine-production-replay-<adapter class>` | #219 gate: genuine `CanonicalGpuField::upload` (LSP, CP), #90 + #207 in one encoder and one submission, 18/18 series and 234/234 members, identities and hashes; audited by [`check_genuine_production_replay.py`](../scripts/precipitation/check_genuine_production_replay.py) |
| `genuine-darain` | WGSL `darain` on the genuine `deac_vals` matches all 234 original results |
| `direct-darain` | All branch, negative and minimum-support cases |
| `production-handoff` | #90 → #207 in one encoder and one submission; #90 rates equal the oracle inputs; members match; typed metadata; device-only pair copy for #119 |
| `fail-closed` | Rejection matrix, device status on overflow, and genuine #202 uploads without, with malformed, mismatched, reassociated or unsupported records; forged mode 2 |

Executed 2026-10-27 on Intel UHD Graphics 620 (Vulkan, `hardware_gpu`) and
Microsoft Basic Render Driver (dx12, `software_wgsl`). Genuine max absolute
difference: 2.3e-7 mm/h (LSP), 6.3e-8 mm/h (CP).

Regenerate the oracle:

```bash
docker run --rm -v <repo>:/workspace/flexpart-gpu -v <flex_extract clone>:/workspace/flex_extract:ro -v <#202 trace dir>:/workspace/trace:ro --entrypoint python3 flex-extract:latest -I /workspace/flexpart-gpu/scripts/precipitation/darain_member_oracle.py --trace /workspace/trace/flex-extract-trace.json --flex-extract-git /workspace/flex_extract --repo /workspace/flexpart-gpu --out /workspace/flexpart-gpu/fixtures/accumulation/precipitation-darain-oracle-v1.json
```

## Genuine #202 production replay (#216/#219)

The genuine example has three same-run accumulations that decrease by GRIB
24-bit packing jitter: LSP cell (1,0) at 30 h and CP cells (1,0) at 30 h and
(2,1) at 45 h (4, 4 and 2 canonical `f32` ulp). flex_extract deaccumulates them
and `darain` clips them to zero. [#216](research/grib-packing-accumulation-216.md)
decided `ADMIT_BOUNDED`; #219 implements it in #75/#90
([contract](accumulation-contract.md#same-run-decreases-from-grib-packing-216)).

The test provider [`tests/support/grib202.rs`](../tests/support/grib202.rs) reads
every record key from the committed GRIB1 bytes, verifies file and message hashes,
packing keys and decoded values against the #216 artifact and the oracle, and takes
`mars_class = OD` from the #202 control provenance recorded in the artifact. The
production test then runs:

1. `CanonicalGpuField::upload` for LSP and for CP (both succeed; 1 and 2 admissions);
2. `prepare_precipitation_members` and one `encode` into a caller-owned encoder:
   #90 LSP, #90 CP, `darain` LSP, `darain` CP, with the #90 buffers bound in place;
3. one submission, the device status check, then readback for evidence only.

Results: every #90 rate equals `max(deac_vals, 0)` under the #71 policy (exact
`+0.0` bits for the 3 admissions, no `-0.0`); 18/18 series; all 13 member
timestamps, four-interval supports and 16 contiguous intervals are exact; 234/234
members equal `genuine.darain_members`. The `darain` kernel is unchanged and adds
no clip. Executed 2026-10-30 on Intel UHD Graphics 620 (Vulkan, `hardware_gpu`;
max member difference 2.6e-7 mm/h) and Microsoft Basic Render Driver (dx12,
`software_wgsl`; 3.2e-7 mm/h).

This end-to-end replay is the #207 closing gate set by #219. The earlier
`genuine-darain` report runs the raw kernel on the oracle `deac_vals` and does
not satisfy that gate. Audit both adapters on a clean commit with:

```bash
python scripts/precipitation/check_genuine_production_replay.py --both target/ci-gate/precipitation-members
```

## Not supported

RRINT=1/`numpf=3` (#212), GRIB2 CP scaling (#211), PUREFC=1, BASETIME
boundaries, ERA5 class `EA` / interval-total sources, #216 admissions outside
GRIB1 `grid_simple`/OD (GRIB2, `D != 0`, other packings), alternate
disaggregation, backward time, `interpol_rain` sampling (#119) and deposition
adoption (#114). No FLEXPART 11.1 precipitation or wet-deposition parity is claimed.
