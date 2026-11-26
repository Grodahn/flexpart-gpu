# ICON-EU precipitation forcing time semantics (#235)

**Decision: `REQUIRES_NEW_ORACLE`.** The legacy `darain` routine can operate on
admitted, contiguous, equal-duration ICON-EU interval means and reproduce
legacy FLEXPART point-member arithmetic. The composed legacy forcing changes
the amount delivered over an ICON interval. It also leaves the hourly to
three-hourly transition without a valid memory pair. The contract blocks
ICON-EU precipitation forcing pending a scientific decision and a reference
that exercises the chosen ICON forcing path.

[Machine-readable contract](../../fixtures/meteorology/icon-eu-precipitation-time-v1/decision.json) ·
[Reference transcript](../../fixtures/meteorology/icon-eu-precipitation-time-v1/evidence.json)

## From DWD accumulations to interval means

`RAIN_GSP`, `RAIN_CON`, `SNOW_GSP` and `SNOW_CON` are accumulated from the
forecast initialization `R`. For valid time `v`, PDT 4.8 encodes accumulation
over the time range from `R` to `v`, with statistical process 1, start lead
zero and range length in minutes. Lead zero is an amount anchor. It has no
preceding interval. #232 owns units, LSP/CP composition, `TOT_PREC` closure,
CCSDS admission and bounded-decrease projection. Apply its rules to each
required endpoint and adjacent pair before reconstructing any member.

Let `A(v)` be the #232 canonical amount at `v` for one component group.
For adjacent admitted endpoints `s, e` of the same run, the amount `Q`
represented by the rate over `(s,e]` is `A(e) − A(s)`, with the existing
#232 positive-zero projection for a certified decrease. The numerical
water-equivalent depth in mm is equal to `Q` in kg m⁻². The interval mean
is `x = 3600 Q / (e − s)` in mm/h, with timestamps in seconds. This pairs
adjacent forecast leads from the publication schedule. A missing hourly
message does not create a longer usable interval. [#232 admission and sources](icon-eu-precipitation-232.md)

## Reconstruction and sampling

The #202 trace of original flex_extract 7.1.2 fixes the meaning of the
member that FLEXPART reads from submember 1 at `T`. For
`RRINT=0`, `PUREFC=0`, BASETIME unset and regular duration `Δ`,
`EcFlexpart.deacc_fluxes` places the output of `darain` at the central
boundary of four interval means. The file/member association has
`stepRange=0`. The direct ICON replay injects those members into the
original rain routine without using an ICON file reader.

| Argument | Physical interval |
| --- | --- |
| `a` | `(T−2Δ,T−Δ]` |
| `b` | `(T−Δ,T]` |
| `c` | `(T,T+Δ]` |
| `d` | `(T+Δ,T+2Δ]` |

The original clips each argument below zero to zero. For admitted ICON
inputs the #232 amount projection already supplies nonnegative rates.
Let `u = b*c/(a+c)` if `a+c>0`, else `b/2`.
Let `w = b*c/(b+d)` if `b+d>0`, else `c/2`.
The point rate is `p(T) = u+w`. Each forecast provides the same mathematical
input type as the ECMWF preprocessing after deaccumulation: means of an
extensive quantity over consecutive equal-duration intervals. This justifies
executing the original algorithm on that finite input class. It says nothing
about subhourly precipitation observed in ICON or whether this legacy
reconstruction is appropriate for wet scavenging with ICON.

After bilinear sampling at a fixed location, let `r₀,r₁` be the sampled
real4 point rates, `dt₁=t−T₀`, `dt₂=T₁−t`, `dt=T₁−T₀`.
Original `interpol_rain` (`eta=no`, `numpf=1`) computes
`(r₀*dt₂ + r₁*dt₁)/(dt/3)` in ordinary memory pairs. At the final exact
timestamp #119 supplies the same final member in both slots. The original
sets `dt=dt₂=1` and keeps `dt₁=0`. Keep the original operation order
and real4 behavior when comparing this primitive. The forward
#227 resident handoff shares the #119 pair across particle lanes.

Even ordinary linear interpolation of `darain` members can change each
interval's amount. In ideal arithmetic its integral at a fixed position is
`Δ_hours * (r₀+r₁)/2`. The original `numpf=1` integral is three times that
expression. For constant positive input `x`, `darain` returns `x`, but
the original integral over one interval is `3*x*Δ_hours` instead of
`x*Δ_hours`. Dividing the members by three would change the already pinned
member contract and would still leave ordinary linear interpolation's
interval residual. These are forcing diagnostics at a fixed grid point,
with no particle motion or scavenging response inferred.

Sources: [#202 original preprocessing trace](precipitation-interval-mapping-202.md),
[#207 member contract](../precipitation-members.md),
[#119 original sampling and source locations](../precipitation-time-sampling.md),
[#227 handoff](../precipitation-resident-sampling.md),
[Tipka et al. (2020), section 3.6.1](https://doi.org/10.5194/gmd-13-5277-2020),
[Hittmeir et al. (2018)](https://doi.org/10.5194/gmd-11-2503-2018).

## Coverage and rejection

These are necessary support conditions for testing the legacy path.
The research verdict also blocks forcing that satisfies every condition.

- For an ordinary pair `T₀,T₁=T₀+Δ`, require every accumulation endpoint
  from `T₀−2Δ` through `T₁+2Δ`: six endpoints, five intervals.
  Require dynamical meteorology at both memory timestamps.
  For a whole simulation, validate the union of the selected supports
  before running. A 15-minute output cadence does not alter the input brackets.
- An exact interior timestamp starts the right pair. Thus at exact interior
  end `E` the next memory is `E+Δ` and its rightmost accumulation is
  `E+3Δ`. At an exact declared final member `E`, use the final duplicate
  configuration with support through `E+2Δ`; the complete window still
  needs at least two genuine members. For an off-grid end, the upper
  member must be the next timestamp and its support extends two more
  intervals. For an off-grid start, use the preceding member and retain
  its two leading intervals.
- On a main cycle initialized at `R`, the first hourly member is
  `R+2 h`. A 48 h simulation beginning at `R` fails at the leading edge.
  Starting at lead 2 h and ending at 50 h uses leads 0–52 h if 50 h is
  the final member, or 0–53 h if 50 h is an interior member. An earlier
  initialization may cover a requested UTC window if it supplies the
  entire window and both halos.
- Main cycles publish hourly through lead 78 h, then every three hours
  through 120 h. The hourly span ends at member 76 h. The three-hourly
  span has its first possible member at 84 h using leads
  78, 81, 84, 87, 90 h. Its first ordinary pair needs lead 93 h as well.
  Its last possible member is 114 h. These are schedule bounds; #232
  closure can further exclude a window. The #232 October sample fails
  closure at 78, 81 and 84 h. A query strictly between 76 and 84 h
  has no supported pair. A simulation crossing the transition is rejected.
  In the shorter intermediate cycles, publishing hourly through 30 h and
  then at 36, 42 and 48 h allows hourly members through 28 h. The
  six-hour segment has fewer than four intervals.
- Reject a missing expected endpoint or component, duplicate or out-of-order
  valid times, wrong time metadata, nonpositive intervals, an interval whose
  duration does not match that forecast segment, or unequal support/cadence
  within a pair. Reject mixed forecast cycles even when valid timestamps
  coincide, and reject missing bracket or halo coverage, extrapolation,
  padding, timestamp shifts and rebinning of the coarse segment. Any #232
  `REJECT` or `UNRESOLVED` endpoint or pair prevents the entire requested
  forcing window. [#230 cadence evidence](icon-eu-provider-230.md)

## Independent replay and handoff

Source: Deutscher Wetterdienst, ICON-EU deterministic regular latitude/longitude
GRIB2, initialization 2026-11-26 00 UTC, leads 0–5 h. The fixture archives
30 unchanged DWD bzip2 objects (CC BY 4.0). The retrieval manifest records
URLs, access timestamps, SHA-256 and lengths of both compressed and decoded
GRIB bytes. The replay verifies every object, reads time identities from
the GRIB octets and requires ecCodes and libaec to agree at every cell. It
uses the frozen #232 component composition and admission routines. The
six complete endpoints are required for the two unpadded hourly members;
the October transition observation comes from the existing #232 evidence.
[Source listing](https://opendata.dwd.de/weather/nwp/icon-eu/grib/) ·
[DWD attribution and terms](https://www.dwd.de/EN/service/copyright/copyright.html)

The reference is the unmodified `Source/Python/Mods/disaggregation.py`
`darain` at flex_extract commit `e0005c99ac81d12faa45a8ff799debbd592b0dc0`.
The loader checks its Git blob then executes that module on four copied
float64 grids for each field and member. The independent comparator is
the analytical equation above. The legacy sampler reference separately
runs the unchanged FLEXPART `interpol_mod.f90:1302–1316,1549–1550` at
`c70586c2b7f5258850705325881c61f557ea9bd8` through the repository's
direct rain driver. It takes the original members rounded to real4,
two genuine 3×3 ICON patches, two positions per patch (node and fractional
position), 15-minute times and an exact final duplicate. The ordinary
pair includes its right endpoint to measure the integral; the duplicate
case exercises the canonical final query at that same endpoint. The
synthetic cloud/temperature fields in the driver only support the isolated
rain call. Both arithmetic comparisons use the existing #71 bound
`|candidate−reference| ≤ 1e−6 + 1e−5*|reference|`. Weather amounts and
rate integrals use their physical units rather than this numerical tolerance.

The original input/output transcript, decoded rows, compiler, source and
executable fingerprints are retained. The analytical comparator matches
original `darain` at all 904,689 cells for both members and fields.
Across 48 precipitation comparisons, the largest absolute error against
original `interpol_rain` is 5.73×10⁻⁶ mm/h, within the #71 combined
tolerance. At the LSP patch's center, lead 2–3 h has a source amount of
21.64258 mm and an integrated original rate of 69.13503 mm. At the CP
patch's center, the corresponding CP amounts are 7.34058 and 20.27651 mm.
Across the grid, even the ordinary linear blend gives a positive
integral in 18,685 LSP cells and 32,905 CP cells whose middle interval
amount is zero after #232 admission. A pinned independent ICON
preparation/consumer reference still has to establish the intended forcing.
FLEXPART 11.1 lacks a reader for this ICON-EU product in the pinned ECMWF/GFS
path, and the published flex_extract input conventions cover ECMWF.

Follow-up boundaries:

1. Specify and pin an independent ICON precipitation preparation and sampling
   oracle. Measure interval amounts, nonnegativity, dry/wet onset and both
   LSP and CP on checksummed data. Evaluate an interval-conserving candidate,
   for example the Hittmeir reconstruction with a separately proven
   FLEXPART submember representation (#212), and resolve the authoritative
   sampling normalization. Compare with original legacy results as controls.
   The forecast interval means alone do not determine the true subinterval
   distribution.
2. Resolve the transition and run-start/end policy for the selected oracle.
   An hourly-only envelope can reject other windows. Mixed-duration or
   boundary reconstruction requires its own evidence with complete forecast
   supports. Do not splice cycles to fill a halo.
3. Keep #240 as the existing #232 admission dependency, then verify the
   eventual prepared ICON fields through the supported production consumer
   and budget gate under #32/#33/#34 as applicable.

## Reproduction

Run the offline lock from the repository with
`python -I scripts/provider/test_icon_eu_precipitation_time.py`. For a fresh
reference run, use the repository's Fortran image, mount the two original
repositories read-only, and work from a clean pinned FLEXPART checkout.
The flex_extract source is read directly from its pinned Git object. The
following Linux shell commands use an unused replay directory under target.

```bash
image=$(docker image inspect flexpart-fortran:latest --format '{{.Id}}')
docker run --rm -v "$PWD:/workspace/flexpart-gpu" \
  -v "$FLEX_EXTRACT:/workspace/flex_extract:ro" -v "$FLEXPART:/workspace/flexpart:ro" \
  -w /workspace/flexpart-gpu --entrypoint bash "$image" -ec '
    d=/workspace/flexpart-gpu/target/issue-235/reproduction
    python3 -I scripts/provider/icon_eu_precipitation_time.py prepare "$d" /workspace/flex_extract
    bash scripts/precipitation/member_mapping_oracle.sh "$d" "$d/oracle"
    python3 -I scripts/provider/icon_eu_precipitation_time.py finish "$d" "$d/evidence.json" "$1"
  ' replay "$image"
```

Compare scientific rows, input hashes and source hashes with the committed
transcript. The report generator checks the prepared case against the exact
driver input and verifies the bytes recorded in the original harness
checksum log. Build and image fingerprints describe the particular execution
environment. A second fresh scratch build reproduced all scientific evidence
and original input/output bytes. Only the compiled executable hash differed.

Provenance: GPT-6 Sol.
