# Settling flag and sticky particle-stop handoff (#203)

This record defines who owns original `lsettling`, the settling velocity and the
per-particle `nstop` at the production Petterssen handoff. It is an
architecture/contract decision. It changes no production Rust or WGSL, fixture,
particle ABI, resident query/publication layout or tolerance. It claims no settling
parity, no new GPU execution proof and no complete transport parity.

The frozen scientific decision is [#180](transport-top-180.md). Its
[fixture](../../fixtures/transport/top-boundary-v1/report.json) remains unchanged.
The versioned machine-readable contract is
[`settling-stop-203-matrix.json`](settling-stop-203-matrix.json) (schema
`flexpart-gpu.settling-stop-contract` v1). Its auditor is
[`check_settling_stop_contract.py`](../../scripts/transport/check_settling_stop_contract.py).

## 1. Original semantics (pinned `c70586c2`)

All locations refer to pristine FLEXPART 11.1 `c70586c2b7f5258850705325881c61f557ea9bd8`.
`advance_mod.f90` has the #180 SHA-256 `7c11a52a…2d68`. The auditor checks each cited line range
against the cited statements.

| Source | Semantics |
| --- | --- |
| `com_mod.f90:175` | `lsettling` is one **run-global** logical, initialized `.true.`. No input option sets it. |
| `readoptions_mod.f90:2133,2574-2585` (`readreleases`) | This is the only mutation. It disables the flag for the whole run when `MQUASILAG` is set and `nspec>1`, or when **any** release point has more than one species with `xmass>1e-9`. A single-species run therefore keeps it `.true.`. |
| `advance_mod.f90:141-151` | Every `advance` call starts with `nstop=.false.` and, if enabled, `settling=0`. Stickiness is bounded by one call. |
| `advance_mod.f90:279-284` | The predictor horizontal update runs first. Only a **domain-exit** `nstop` from `update_xy` returns early. Then comes the first `pushpartdown`. |
| `advance_mod.f90:299-334` | The correction gates are a full sync step, advanced time within memory and the same grid. **None tests `nstop`.** A boundary-set stop is still corrected. |
| `advance_mod.f90:398-423`, `583-598` | For the velocity, `mdomainfill=0` and `lsettling` select the first species with `xmass(npoint)>tiny(1.0)`. Only if `density>0` does the code call `get_settling`. A forward-only ground clamp follows, then `w=w+settling`. A density≤0 (gas) carrier keeps velocity 0. |
| `advance_mod.f90:706-772` | The corrector samples the advanced signed time at the **stored predictor** position, recomputes settling there and applies the half-difference to that predictor. Then the second `pushpartdown` runs. |
| `advance_mod.f90:889-895` | Non-ETA: `z >= height(nz)` (inclusive, f64 vs. f32 endpoint). If `ldirect==-1 .and. lsettling`, it sets `nstop` and **retains z**. Otherwise it places at the f32 `height(nz)-100*eps`. It never clears `nstop`. |
| `advance_mod.f90:624-625` | Adaptive PBL substeps also call `pushpartdown`. This is an unsupported route. |
| `settling_mod.f90:162-192` | `get_settling` uses nearest-column T/ρ from memory slot `1`. Linear vertical weights extrapolate above the top level (`indz=nz-1`). |
| `timemanager_mod.f90:516-541` | A stopped particle skips `partpos_avg`/`calcfluxes`. After the parallel loop, each flagged live particle is terminated and `ialive` is rebuilt **before** the deposition/decay loop (`:562`). |
| `particle_mod.f90:357-381` | `terminate_particle` sets only `alive=.false.`, `tend`, and the counters. Mass and position are untouched. Nothing is deposited. |
| `output_mod.f90:125-128,667-669` | Initial-condition attribution and `conccalc` iterate live particles only. |

The states must not be collapsed:

| State | Original consequence |
| --- | --- |
| Settling disabled | Ordinary placement at both actions, both directions. |
| Enabled, zero instantaneous velocity (density≤0, or `mdomainfill≠0`) | No velocity term. A backward particle at or above the top **still stops**. |
| Enabled, nonzero velocity | `w+vs` at both stages (vs recomputed at the predictor). Backward stop at the top. |
| Predictor encounter | Stop, z retained unplaced. Advanced sampling and correction use the retained z. |
| Corrector encounter | Second action stops and retains the corrected z. |
| Previous sticky stop | Unreachable at `advance` entry. The particle was already terminated and is not iterated. |
| Fatal meteorology | `getfields` `error stop` (`timemanager_mod.f90:218`) is a run failure, never a particle fate. |

Forward transport never stops at the top, with or without the flag. The flag is never derived
from `velocity != 0`. Direct #180 rows confirm the predicate and the backward scenario-9 stop.

## 2. Current production owners and gaps

The matrix `owners` table records each owner's path, symbol, invariant, gap, producer and consumer.
It also records lifetime, validation side and assignment. The auditor verifies that every path and
symbol still exists. In summary:

| Component | Current owner / API | Gap and assignment |
| --- | --- | --- |
| Carrier parameters | `config::SpeciesConfig` (#10) | `BackwardReceptorConfig` binds no carrier, only slot-0 `mass_kg`. → #214 |
| Enablement | `ForwardStepForcing::settling_enabled` (per-step bool, default `false`, rejected when `true`) | It has the wrong lifetime and no provenance, and its default is **not** the original default. → #214 |
| Velocity | `gpu::settling::encode_settling_velocities`, `SettlingQuery` (#35, isolated) | No device-built query at both stage positions and no production consumer. → #35 physics, #37 coupling |
| Gas carrier | `SettlingCarrier::from_species_config` rejects density≤0 | In the original this is a zero-velocity branch, not an error. It needs a typed selection. → #214 |
| T/ρ inputs | Host-supplied query fields | No owner reproduces nearest-column, slot-1, extrapolating sampling at both stages. #37 is scoped to **forward** only. → backward coupling is a recorded blocker for #37 |
| Time/source identity | `MetTimeBracket` canonical selections (#112/#89/#197) | The producer must bind the same bracket, selections and step. → #37, checked by #214 |
| Particle identity/ABI | 96-byte `Particle`, lane index, `release_point` | None. The ABI is unchanged. |
| Liveness | `FLAG_ACTIVE` | None. It is the existing representation of `alive`. |
| Per-particle stop | none | Missing a step-private stop word in `ResidentAdvectionStep`. → #214 |
| Fatal status | six sample regions plus arithmetic word, `require_success` | Must stay fatal-only. The producer gets its own fatal region. |
| Predictor/corrector | `advection_predictor_query.wgsl`, `advection_corrector.wgsl` (#198, exact hand-off from #210) | No flag predicate and no retained-z path. → #214 |
| Eligibility | `advection_step_guard.wgsl` | Must terminate stopped lanes on success. → #214 |
| Compaction | `encode_compaction_with_reorder`, forward only; host count after the checkpoint | None in the envelope. Backward compaction stays unsupported. |
| Publication | `advection_step_commit.wgsl` raw-word copy | None. Stopped lanes are published by the same copy. |
| Attribution | `collect_source_collections` skips inactive lanes after `require_success` | None. |
| Mass ledger | `Particle::mass` | A category for mass on terminated particles. → #38 |

**Production fidelity gap (matrix C20).** The backward driver uses the ordinary action. It has no
derived flag. Its receptors are single-species without MQUASILAG, so the original would derive
`lsettling=.true.` and stop particles at the top. Current backward production therefore matches the original only for
particles that never reach the transport top. This record does not change the driver. Issue #214
must derive the flag and then either execute the stop route or fail closed. It must never keep the
silent `false` default.

## 3. Minimal typed handoff (proposed, not implemented)

```rust
/// Run-owned original `lsettling` (readoptions_mod.f90:2574-2585). Never inferred from velocity.
pub(crate) struct TransportSettlingRoute {
    enabled: bool,
    direction: TransportDirection,          // ldirect, fixed for the run
    provenance: SettlingRouteProvenance,    // normalized release-inventory and #10 carrier hashes,
                                            // MQUASILAG=0, pinned rule anchor
}

/// Vertical settling term for both Petterssen stages.
pub(crate) enum PetterssenSettlingInput<'a> {
    Disabled,                                     // route disabled: no term
    ZeroByCarrier(CarrierSelectionIdentity),      // enabled; every live lane's selected carrier has density<=0
    Produced(PetterssenSettlingVelocities<'a>),   // #35/#37 producer; rejected until accepted
}

pub(crate) struct PetterssenSettlingVelocities<'a> {
    current: &'a wgpu::Buffer,    // per-lane m/s at the current position/time
    predictor: &'a wgpu::Buffer,  // per-lane m/s at the stored predictor, encoded between the stages
    status: &'a wgpu::Buffer,     // producer fatal region
    identity: SettlingProducerIdentity, // context, shader/carrier/meteorology hashes, bracket and
                                        // time selections, capacity, active prefix, step, signed dt
}
```

- The route is built once per run from the normalized release inventory and the #10 carrier
  table. It is bound beside `SharedHeightTransportBoundary` and is never re-derived per step.
  `ForwardStepForcing::settling_enabled` stays a rejection-only request until it is removed.
- `ZeroByCarrier` adds nothing. The arithmetic must be bitwise the disabled arithmetic.
- `Produced` needs device-side query construction at the stored predictor between the stages,
  in the caller's encoder. That is #37's coupling. The #35 kernel already records into a caller
  encoder.
- The single device flag `stop_route = enabled && direction == backward` goes into the private
  Petterssen `Params` uniform. Since #210 it carries spare pad words; the corrector already consumes the predictor's private raw height. `Particle`, the resident
  query lane and the raw-word commit layout are unchanged.
- The stop storage is one step-private `u32` per lane in `ResidentAdvectionStep`: bit 0 for a
  predictor stop, bit 1 for a corrector stop. The original `nstop` equals `stop != 0`. The buffer
  is cleared at encode and written only with OR. It is retained with the deferred checkpoint like
  `status`. It is never part of the particle buffer.

### Producer/consumer flow

```text
#10 SpeciesConfig ─┐
release inventory ─┴─> TransportSettlingRoute (run) ──> stop_route flag ─┐
                                   │                                    │
#35 kernel <── T/rho queries (#37) │ carrier selection                  v
     │                             └─> PetterssenSettlingInput ──> predictor ──> advanced samplers (#87-#89)
     └──── current/predictor m/s, producer status ───────────────────────┘            │
                                                                                        v
canonical bracket (#112) ──> samplers ──> status regions        corrector ──> stop words (step-private)
                                              │                      │               │
                                              v                      v               v
                                     guard: fatal => all lanes inactive; else stop => FLAG_ACTIVE cleared
                                              │
                                              v
                    Langevin / deposition / decay (skip inactive) ──> [forward compaction] ──> commit (all-or-nothing)
                                              │
                                              v
                    require_success ──> backward readback ──> collect_source_collections (active only)
```

## 4. Lifecycle through the existing stages

| # | Stage | Original obligation | Candidate detail |
| --- | --- | --- | --- |
| 1 | Host preflight | — | Validate route, carrier selection, producer identity, context, capacity, prefix, bracket and step **before** release/encoding (typed errors). |
| 2 | Acquire state | `nstop=.false.`, settling 0 | D2D snapshot to staged state; clear status **and** stop words. Live lanes = `FLAG_ACTIVE`. |
| 3 | Predictor | current wind (+vs) | Unchanged sampling; `ZeroByCarrier` adds nothing. |
| 4 | First action | inclusive test; stop iff backward with flag | `stop_route`: set bit 0 and keep the raw z. Otherwise ordinary f32 placement as in #198. |
| 5 | Preserve predictor | stored position feeds the correction | The #210 private record keeps the raw predictor height. The advanced query height is the raw height for a stopped lane and the placed height for an ordinary crossing. |
| 6 | Corrector | always runs when the gates pass; vs recomputed at the predictor | Base is the raw record if stop bit 0 is set, the placed height for an ordinary crossing, else the raw record. The corrector reads bit 0 and never re-tests `raw >= top`, so both stages share one decision (#210). A stopped lane is still sampled and corrected. |
| 7 | Second action | inclusive test; never clears | `stop_route`: OR bit 1 and keep z. Otherwise ordinary placement. |
| 8 | Stage result | — | Corrected bytes in staged state; stop words private. |
| 9 | Fatal observation | — | Existing guard: any fatal region inactivates **all** private lanes. Stop words are not status. |
| 10 | Termination | `terminate_particle` before deposition/decay | Guard on success: clear `FLAG_ACTIVE` on lanes with `stop != 0`. Position and mass are retained. |
| 11 | Downstream | stopped particles excluded | Langevin/deposition/decay already skip inactive lanes. Forward compaction cannot meet a stop. |
| 12 | Publication | — | Existing raw-word commit only when every status is zero. A stopped lane is published inactive. |
| 13 | Attribution | live particles only | Backward `require_success`, then readback, then `collect_source_collections` skipping inactive lanes. |

There is no new sampler, no second status hierarchy, no intermediate readback and no CPU fallback.
The existing samplers have no above-top lane rejection: the adapter checks finiteness and grid, and
the vertical stage clamps to the highest level. Whether that clamping reproduces
`interpol_wind_short` above the top is owned by #87-#89. #214 must prove it with genuine
rows; on a mismatch it stops and records the dependency. The GPU applies turbulence after
advection, so a stopped lane does not receive that step's downstream turbulence. No supported
consumer reads terminated positions; particle dumps of terminated particles remain unsupported.

## 5. Stop versus fatal

| Class | Where | Effect |
| --- | --- | --- |
| Scientific stop | stop word | The lane terminates; the step still succeeds; other lanes are unaffected. |
| Invalid settling input | host | Typed error before release/encoding. |
| Missing producer/resource | host | Typed error before release/encoding (today's rejection). |
| Unsupported configuration (MQUASILAG, domain fill, ETA, PBL substep, restart, nest, global, backward compaction) | host | Typed error before release/encoding. |
| Context/resource identity mismatch | host | Typed error before encoding. |
| Meteorology/sampler failure | device status region | Every lane inactive, no commit, `require_success` error. |
| Settling producer failure | its own device status region | The same as meteorology. It is never a stop. |
| Corrected arithmetic failure | arithmetic word | The same. |

The producer region joins the same fatal-region scan in the corrector, guard, commit and
`require_success`, so none of them can miss it. A stop never writes a status word, and
`require_success` never reads stop words. A stopped lane
is still validated: a nonfinite corrector sample on that lane is fatal. Correction runs for
stopped particles in the original, so a stop cannot excuse a failure.

## 6. Publication and compaction

The boundary between staged and committed state is the existing commit dispatch. Before it,
staged lanes and stop words are transaction state. If any status word is nonzero, the commit
copies nothing: no positions, no `FLAG_ACTIVE` change and no stop. Every original byte is
untouched, including tails and padding. The stop words are discarded with the step. Forward
compaction runs on private state, and its host count is accepted only after the status
checkpoint. Stops are backward-only, and the backward driver has no compaction, so compaction
never meets a stop in the supported envelope. A future backward compaction request must be
rejected until it proves the same order. Termination is not deletion, mass removal or
deposition. Host slot reuse of terminated lanes is not required.

## 7. Acceptance matrix

The matrix has 25 versioned cases: the 19 required by #203 (C01–C19), plus C20–C25 for the
default-flag gap, multi-species disable, forward with the flag, PBL substeps, horizontal domain
exit and f64-only probes. Each case names its original anchors and branch, input state,
predictor/corrector behavior, `nstop` transition, downstream eligibility, GPU classification,
evidence surface and status. Oracle evidence is recorded only as **selectors** into the frozen
#180/v1 rows and the genuine [#213/v2 rows](../../fixtures/transport/top-boundary-v2/report.json) (count, `nstop`, nonzero settling, f32 representability). The auditor resolves them
and rejects any numeric expectation. Status summary:

- **Supported now:** C01, C02 (ordinary action; C02 holds only when the run genuinely derives `.false.`).
- **Rejected now:** C16. **Unsupported:** C15, C23–C25. **Unreachable in the original:** C10.
- **Implementation-blocked** (semantics frozen): C12–C14, C17–C21.
- **Original evidence supplied by #213; implementation-blocked by #214:** C03, C05–C09, C11.
  `FLAGGED_ADVANCE_V2` selects genuine v2 public-advance rows for all eight frozen scenarios,
  both directions, geometries and epochs. Original lane values also support C12–C14, C17,
  C18 and C20; their structural/device/driver proof obligations remain unchanged. C22 gains
  forward zero-carrier evidence; its nonzero coupling remains producer-blocked.
- **Producer-blocked:** C04 (genuine backward scenario-9 rows exist; they need #35 plus backward
  coupling) and C22 (#37).

## 8. Implementation split

- **#35** owns the original-conformant `get_settling` velocity and density>0 carrier validation.
- **#37** owns production coupling. That covers per-lane carrier selection for density>0, T/ρ at
  both stage positions, the forward ground clamp, dry deposition and mass. Its claim is
  forward-only. Backward velocity coupling (needed by C04) is a recorded gap for #37's owner.
- **#38** owns the ledger category for mass on terminated particles.
- **Oracle extension, [#213](https://github.com/Grodahn/flexpart-gpu/issues/213).** It produces genuine flagged zero-velocity public-advance rows
  for scenarios 1–8 in both directions from the unchanged #180 driver/launcher, as a new fixture
  version. v1 stays frozen. The [executed proof and #214 consumption instructions](flagged-top-213.md)
  document the 64 genuine rows and observation boundaries.
- **Implementation, [#214](https://github.com/Grodahn/flexpart-gpu/issues/214).** It covers the run-owned route, `ZeroByCarrier`, the step-private
  stop word, both actions, guard termination, atomic publication and the C20 production gap. It
  is a **separate issue that blocks #37's backward enablement**, not a child of #37. The stop
  state lives in transport-owned `ResidentAdvectionStep` and the #198 shaders, not in #37's
  deposition/mass surfaces. The zero-velocity route needs neither #35 nor #37, and it already
  affects every default single-species backward run. Nonzero velocity (`Produced`) stays rejected
  until #35/#37 deliver.

The implementation must require genuine pinned rows (#180 plus the extension), real forward and
backward driver execution on software and hardware WGSL, and exact raw per-lane fate/stop words
with published bytes. It must also prove sticky and terminated behavior across two steps,
compaction rejection, fatal atomicity with a stopped peer, and source/target/shader/adapter/oracle
hashes, with no CPU fallback and no new tolerance.

## Unsupported envelope

Nonzero settling velocity in either direction (until #35/#37), adaptive PBL substeps, ETA/`zeta`,
domain fill, MQUASILAG, restart (`ipin` 3/4), nested/global/polar grids, backward compaction,
f64-only predicate probes and particle dumps of terminated particles remain unsupported. The
horizontal domain-exit `nstop` is a separate fate cause with no owner. The existing horizontal
clamping is retained and is not addressed here.

## Audit

```text
python scripts/transport/check_settling_stop_contract.py --checkout <pristine-flexpart-pin>
python scripts/transport/test_check_settling_stop_contract.py
```

The auditor requires the pinned clean checkout and the #180 `advance_mod.f90` hash. It verifies
all source anchors, the frozen raw-output hash, every oracle selector against the frozen rows,
case consistency rules and every repository owner symbol. `--allow-missing-checkout` reports
`PASS_SOURCE_ANCHORS_NOT_CHECKED`, never `PASS`. Its verdict is
`CONTRACT_ONLY_NO_SETTLING_OR_TRANSPORT_PARITY`.


## Production implementation follow-up (#214)

The frozen findings, matrix classifications and original fixtures above retain
their historical meaning. The zero-by-carrier route, step-private stop lifecycle
and both production drivers are now implemented in the
[#214 integration](../transport-top.md#settling-stop-integration-214).
Acceptance requires the extended real-device matrix and independent evidence
auditor; this implementation-status link does not promote nonzero velocity,
#204 trajectory diagnostics or whole-FLEXPART parity.
