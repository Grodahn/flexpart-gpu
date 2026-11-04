# Pair-shared resident precipitation sampling (#227)

#227 lets every resident particle lane sample the #207 LSP/CP point rates. It
uses the unchanged #87 spatial arithmetic and the unchanged #119 time arithmetic.
The [#119 batch API](precipitation-time-sampling.md) stays the bounded
validation path. This stage adds no new scientific semantic.

```text
#207 members (encoded once) -> 4 device plane copies per step
#171 lanes -> #171 adapter -> #87 (x4) -> lane-parallel #119 -> fail-closed publish
```

## Scope

The supported envelope is the #119 envelope: forward time, `numpf=1`, `RRINT=0`,
`PUREFC=0`, BASETIME unset, regional geometry. All lanes of one step share
`itime`. `select_precipitation_memory_pair` therefore runs once on the host
control plane, with the unchanged exact-interior (left-inclusive) and
final-timestamp rules. Out of scope:

- per-particle `itime`;
- nested or periodic grids;
- `numpf=3`/`RRINT=1` (#212);
- backward time;
- wet scavenging and cloud logic (#33);
- production wet-deposition wiring and parity (#34);
- provider decoding (#32).

Per the stop rule, any of these needs its owning decision issue first.

## API and lifecycle

Owner: [precipitation_resident.rs](../src/gpu/precipitation_resident.rs).

1. Encode the #207 owner once: `PreparedPrecipitationMembers::encode`. Steps
   borrow the owner read-only, so nothing can re-encode it while a step lives.
   Before any encode, the owner is rejected. Until its encoded work executes,
   #207 status keeps `STATUS_NOT_EXECUTED`, so zero-filled members fail closed.
2. Once per context: `ResidentQueryKernels::new` and
   `ResidentPrecipitationKernels::new`.
3. Per step: `prepare_resident_precipitation_rates(ctx, &producer, &mut batch,
   itime, 1)`. It validates direction, context, encoded producer, grid identity
   and the complete #207 window, selects the pair and checks the integer time
   differences. It then allocates four device-only #87 planes and
   O(capacity) lane resources. It uploads no field values and no coordinates.
4. `encode_from_particles(ctx, &particles, &resident, &kernels, &mut encoder)`
   records the following into the caller encoder:
   - #171 status reset and particle producer;
   - for each of LSP-left, LSP-right, CP-left, CP-right: a device plane copy
     (`plane_byte_range`), the #171 adapter (per-lane domain/corner status) and
     #87;
   - the lane-parallel #119 kernel;
   - the publication gate.

   Nothing is submitted, polled or read back. A step encodes once.
5. The caller submits. `download` is a validation boundary only.

The kernels are [rates](../src/shaders/precipitation_resident_rates.wgsl)
and [publish](../src/shaders/precipitation_resident_publish.wgsl). The rates
kernel ports `interpol_mod.f90:1302-1316,1549-1550` with the operation order
of `precipitation_temporal.wgsl`:

- `dt/3` is kept;
- at the final timestamp only, `dt=dt2=1` and the result is divided by the
  rounded `f32(1/3)`;
- products are separate, with no FMA.

`dt1/dt2/dt` are one uniform per step.

## Output and status

`rates_mm_per_h` is interleaved `[LSP, CP]` `f32` mm/h per capacity lane.
`status` is the #171 word array: a shared fatal word, then one lane reason.

| Shared bit | Meaning |
| --- | --- |
| 1 | Any failed #171/#87/#119 lane (existing #171 bit) |
| 4 | Non-finite #87 sample or #119 result (lane reason 9) |
| `<< 8` | Nonzero #207 producer status, shifted |

Lane reasons keep the #171 ABI:

| Reason | Meaning |
| --- | --- |
| 0 | Inactive or tail |
| 2 | Valid |
| 3 | Negative cell |
| 4 | Invalid fraction |
| 5 | Non-finite query |
| 6 | Outside the #87 domain |
| 8 | Non-finite member corner |
| 9 | Incomplete or non-finite result |

Publication is a separate dispatch, so it sees every lane's final status. A
lane is published only when the shared word is zero and the lane is valid.
Inactive, tail and failed lanes, and every lane after any failure, become a
quiet NaN, never a dry-weather zero. `download` rejects:

- unencoded or unexecuted steps (a device-written completion word);
- foreign contexts;
- any nonzero shared or #207 status;
- inconsistent lane status or values.

## Scalability

Per step, device memory is `4 x cell_count x 4 B` for the shared planes plus
164 B per lane plus fewer than 128 B of control buffers. It is reported as
`ResidentPrecipitationMetadata::device_bytes`. The 164 B per lane are:

- four #87 query/sample pairs;
- the adapter height;
- the candidate and published `[LSP, CP]` values.

Encoding is a constant 12 dispatches plus 4 copies, independent of capacity.
Each dispatch builds one bind group, never one per lane. For comparison, the
batch API for the same 588 queries allocates `588 x 4 x cell_count x 4 B`.

## Validation and retained evidence

Validation levels: isolated kernel, resident composition, and pinned-oracle
replay. No wet-deposition consumption or parity is claimed.

- **Isolated kernel:** `cargo test --lib precipitation_resident`. Synthetic
  lane samples cover interior, boundary and final-timestamp arithmetic under
  the #71 policy. Injected NaN, overflow, pending lanes and a #207 failure
  publish no lane.
- **Resident composition:** the [device test](../tests/precipitation_resident_gpu.rs)
  uploads the genuine #202 GRIB fields and encodes #90/#207 once. It then runs
  49 resident steps, one per original timestamp, with 16 lanes each. Each step
  has 12 position lanes, flag-inactive lanes 3 and 8 (NaN coordinates), and
  active-flagged NaN tail lanes 14 and 15. A 4096-lane step at the same time is
  included. All of this is one encoder and one submission.
- **Bitwise identity:** all 588 lanes equal the #119 batch API on the same
  queries, bit for bit. The rows split into interior (432), exact interior
  member (144) and final (12).
- **Pinned original:** every row matches
  [the pinned original fixture](../fixtures/interpolation/precipitation-temporal-v1.json)
  under `|c-o| <= 1e-6 + 1e-5|o|`.
- **Fail-closed matrix:**
  - foreign contexts, kernels and batches;
  - unencoded #207 owner;
  - backward and invalid directions;
  - outside-coverage times;
  - grid/source mismatch;
  - `numpf=3`/`RRINT=1`;
  - unsubmitted step;
  - on-device #87 domain, non-finite, negative-cell and fraction lanes;
  - #207 overflow.

  Each one rejects the step and leaves all rates NaN.

Reports are written to `target/ci-gate/precipitation-resident/report-<adapter
class>.json`. Each contains:

- candidate revision and clean flag;
- adapter;
- shader, source and input hashes;
- #207/#216 identities;
- per-step pair, time differences, capacity, prefix and lane status;
- the allocation audit;
- bitwise classes;
- #91 `GpuCalculationEvidence`;
- member bytes for the fresh replay.

```text
cargo test --lib precipitation_resident -- --test-threads=1
cargo test --test precipitation_resident_gpu -- --test-threads=1
python scripts/precipitation/check_genuine_production_replay.py --resident target/ci-gate/precipitation-resident/report-software_wgsl.json --software
python scripts/precipitation/test_check_precipitation_resident.py
```

Repeat with `FLEXPART_GPU_SOFTWARE=1` for the software WGSL adapter. The fresh
matched-input original replay reuses the unchanged #119 oracle path:

```text
python scripts/precipitation/temporal_oracle.py run target/ci-gate/precipitation-resident/original-hardware --checkout <pristine-flexpart> --device-report target/ci-gate/precipitation-resident/report-hardware_gpu.json --output target/ci-gate/precipitation-resident/matched-hardware.json
python scripts/precipitation/check_genuine_production_replay.py --resident --both target/ci-gate/precipitation-resident
```

`--both` requires both adapter reports and both `matched-*.json`, from the same
clean candidate. It fails on:

- missing execution or evidence;
- stale revisions;
- changed inputs or member bytes;
- bitwise drift from #119;
- a memory bound above the formula;
- particle-scaled dispatches;
- published inactive lanes;
- original mismatches.
