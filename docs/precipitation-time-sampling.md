# Precipitation point-rate time sampling (#119)

Provenance marker: **GPT6.1 Sol**.

The production preparation is now defined and implemented: #202 decided that
direct #90 interval means are **INCOMPATIBLE** with FLEXPART precipitation
members; #207 reconstructs the required point rates with GPU `darain`; #219
proves genuine accumulated GRIB1 through #90 and #207. #119 adds the final
spatial and temporal sampling composition. It does not change those stages.

```text
CanonicalGpuField::upload -> #90 interval rates -> #207 point members
    -> #87 spatial samples -> #119 temporal precipitation rates [mm/h]
```

## Scientific contract

Authority: unchanged FLEXPART 11.1
`c70586c2b7f5258850705325881c61f557ea9bd8`, `eta=no`, `numpf=1`, regional
geometry. The [#202 decision](research/precipitation-interval-mapping-202.md),
[#207 handoff](precipitation-members.md) and [#216 admission](accumulation-contract.md#same-run-decreases-from-grib-packing-216)
remain authoritative upstream.

`RRINT=0`, `PUREFC=0`, BASETIME unset, forward time only. Each member at `T`
is a reconstructed **point rate in mm/h**, with support `[T-2Δ,T+2Δ]` and
four original interval identities. Source snapshots, distinct LSP/CP identities,
grid, calendar, context and #216 admission records remain attached. No raw
interval mean, amount, shifted member, padding or CPU reconstruction is accepted.

For `N >= 5` contiguous intervals from `s0`, with one positive integer-hour
`Δ`, genuine members are `T_j=s0+jΔ`, `j=2..N-2`. Queries must satisfy
`T_first <= itime <= T_last`. Interior selection is left-inclusive and
right-exclusive: an exact interior timestamp begins the next pair. The whole
member window, including all four-interval supports, is checked before encoding.
There is no extrapolation outside coverage or leading/trailing zero fill.

## Original arithmetic and source crosswalk

`getfields_mod.f90:159-203,243-287` selects and rotates the memory pair.
`windfields_mod.f90:538-559,2063-2066,2180,2335-2348` loads real4 precipitation
into submember 1. `wetdepo_mod.f90:290-327` passes the actual `itime` without a
precipitation-specific shift. #114 retains responsibility for that consumer.

The [new shader](../src/shaders/precipitation_temporal.wgsl) ports
`interpol_mod.f90:1302-1316,1549-1550`:

```text
dt1 = real(itime-t0)
dt2 = real(t1-itime)
dt  = real(t1-t0)
if dt == 0 and dt1 == 0: dt=1; dt2=1
dtt = dt / 3
result = (r0*dt2 + r1*dt1) / dtt
```

Differences are checked integer seconds before GPU conversion to `f32`, avoiding
loss of epoch precision. Products, sum and division retain the original order;
there is no FMA call or ordinary #89 blend. The factor of three for ordinary
pairs is preserved, without upstream compensation. At `itime=T_last`, both
memory slots select the same genuine final member and execute division by the
rounded `f32` value `1/3`. This is not replaced by multiplication by three.
Every other duplicate-time configuration is rejected.

## API, ownership and failures

Owner: [precipitation_temporal.rs](../src/gpu/precipitation_temporal.rs).
`PrecipitationTemporalKernels::new(ctx)` reuses the existing runtime and
#207/#87 kernels. `prepare_precipitation_time_samples(ctx, &mut producer,
queries, 1)` accepts only a genuine prepared #207 owner, validates coverage
and coordinates, and allocates private dispatch resources. There is no API
to recombine arbitrary member buffers or provenance.

`plan.encode(ctx, kernels, encoder)` records #90, #207, four #87 samples per
query and #119 into the caller-owned encoder. It uses `member_at(T)` and
`plane_byte_range(member)`, with encoder-local plane copies into existing #87
resources. #87 supplies the indices, fractions and spatial weights. No host
samples a precipitation field or downloads an intermediate. Output is an
interleaved `[LSP,CP]` storage buffer in mm/h, with typed source/query/pair
metadata and the shared fatal status. A batch is encoded once; retain its
owner, sources and kernels through submitted completion.

The caller owns device error scopes, submission and completion. Discard an
encoder after any encoding error. Every device consumer must honor the shared
status: any nonzero status invalidates **the entire batch**, including any
otherwise finite prefix. #207 bits 1/2 retain producer failures; bit 4 reports
non-finite #87/#119 results. Failed lanes contain NaN, never a valid dry-weather
zero. Explicit `download` rejects unencoded/unsubmitted work, nonzero status,
other contexts and non-finite output. There is no mandatory synchronization
between stages and no CPU fallback.

## Validation and retained evidence

[Original fixture](../fixtures/interpolation/precipitation-temporal-v1.json):
the existing [unchanged rain driver](../scripts/interpolation/direct_interpolation_oracle.f90)
linked through [direct_oracle.sh](../scripts/interpolation/direct_oracle.sh),
using real4-rounded observed #207 oracle members. All 13 genuine memory
configurations are exercised: 12 ordinary pairs at four times and 12 positions,
then the final duplicate at 12 positions. This gives 588 queries / 1,176 LSP/CP
comparisons, covering every node, fractional spatial gradients, non-midpoint
times, exact boundaries, zeros and wet/dry transitions. Raw matched inputs and
outputs, source/driver/executable hashes, timestamps and units are retained.
Raw harness hashes retain the executed bytes. The auditor recognizes only LF
and CRLF checkout encodings of identical harness content, for Windows/Linux
portability; upstream source and scientific input/output hashes stay exact.

The historical `rain-layer-fields` golden and five probes
`(9.75,1.5)`, `(84.75,2.25)`, `(159.75,3)`, `(234.75,3.75)`, `(309.75,4.5)`
remain isolated arithmetic controls. They make no production-member mapping claim.

The genuine [device test](../tests/precipitation_temporal_gpu.rs) uploads the
committed #202 GRIB bytes with #219 records and executes the entire chain using
one caller encoder and one submission, with no intermediate host readback.
It retains all 234 final member bytes **after** final sampling for a second,
exact matched-input original-routine replay. That validation readback never
feeds a production GPU stage. Reports use #91 `GpuCalculationEvidence`; the
[extended #219 auditor](../scripts/precipitation/check_genuine_production_replay.py)
recomputes comparisons under the unchanged #71 rule
`|candidate-oracle| <= 1e-6 + 1e-5*|oracle|`.

```text
cargo test --lib precipitation_temporal -- --test-threads=1
cargo test --test precipitation_temporal_gpu -- --test-threads=1
python scripts/precipitation/check_genuine_production_replay.py --temporal target/ci-gate/precipitation-temporal/software_wgsl.json --software
python scripts/precipitation/test_check_precipitation_temporal.py
```

Repeat on hardware and software WGSL adapters. Reports are
`target/ci-gate/precipitation-temporal/{hardware_gpu,software_wgsl}.json`.
Fresh exact-input replay (use a different output directory for each adapter):

```text
python scripts/precipitation/temporal_oracle.py run target/ci-gate/precipitation-temporal/original-software --checkout <pristine-flexpart> --device-report target/ci-gate/precipitation-temporal/software_wgsl.json --output target/ci-gate/precipitation-temporal/matched-software.json
python scripts/precipitation/check_genuine_production_replay.py --temporal target/ci-gate/precipitation-temporal/software_wgsl.json --software --matched target/ci-gate/precipitation-temporal/matched-software.json
```

Use `--oracle-directory <existing validated scratch oracle directory>` to reuse
the #202 scratch objects; the unchanged direct harness rechecks link inputs.
Both adapter classes and their matched-input replay must pass before #119 can
close. Audit the combined acceptance with
`python scripts/precipitation/check_genuine_production_replay.py --temporal --both target/ci-gate/precipitation-temporal`
after retaining `matched-hardware.json` and `matched-software.json` there.
No execution, missing evidence, stale revisions, missing pairs, changed
inputs, mismatches or intermediate host transfer can pass the auditor.

Negative coverage combines the #119 query/window/context and fatal-device tests
with existing #207/#219 source/preparation tests: missing leading/trailing
support, gaps, unequal/nonpositive spacing, ordering/duplicates, invalid units,
raw means/amounts, source/grid/calendar/context differences, invalid #216 records,
non-finite values, invalid coordinates, backward direction, RRINT=1, numpf=3,
PUREFC=1, BASETIME and missing/unexecuted resources. The auditor's mutation tests
also reject absent execution/evidence and altered provenance/results.

## Superseded draft and downstream scope

[PR #205](https://github.com/Grodahn/flexpart-gpu/pull/205) is historical
documentation-only research. Its original-source crosswalk and five direct
probes are valid and incorporated here. Its statement that #202 is unresolved
is superseded by #202, #207 and #219. Close that draft as superseded when the
replacement #119 implementation PR is available; do not merge it unchanged.

The batch API is sized for bounded validation batches, not particle-scale
queries: each query owns four full-grid #87 plane buffers, one query buffer and
four copy, four #87 and one #119 dispatch. Device memory therefore grows as
`queries x 4 x cell_count x 4 bytes`, and the API intentionally does not share
planes between queries with the same memory pair.

#114 still owns wet-deposition consumer migration, resident particle query
adoption (including a pair-shared, particle-scale query layout) and honoring
the fatal status before applying forcing. Cloud-mask
physics, deposition rates (#33), numpf=3 (#212), provider decoding (#32) and
full wet-deposition/FLEXPART parity are outside this change.
