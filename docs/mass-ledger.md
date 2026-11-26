# Forward mass ledger v1

Issue #38 owns the inventory identity, transfer ownership and observation
boundaries for forward FLEXPART-GPU point releases. The scientific authority is
FLEXPART 11.1, revision `c70586c2b7f5258850705325881c61f557ea9bd8`. The
[reference crosswalk](../fixtures/mass-ledger/reference-v1.json) records immutable
Git blob identities and line anchors. The [run schema](../schemas/mass-ledger-v1.schema.json)
and [Rust audit](../src/validation/mass_ledger.rs) implement the stable-species
evidence contract. The current production driver has the integration gaps listed
below.

## Reservoirs and transfers

All inventories and transfers are in kilograms of the configured species, bound
to the immutable #10 species slot order. The run-wide sum also uses kilograms.
Each boundary has a transfer record and the independently observed airborne
stock after each phase, per species. Named sink and injection totals accumulate
from run start. The airborne stock is a snapshot of live particles.

| Quantity | Owner and semantics |
| --- | --- |
| Requested | Nominal configured mass for the entire run. This is a source diagnostic outside the conservation sum. Unscheduled or unspawned mass stays outside injected inventory. |
| Injected | Cumulative mass actually assigned to spawned particles after the mass-unit conversion and represented particle split. Injection credits airborne by the same amount. The audit does not infer injection from requested mass. Release scheduling and duplicate spawn checks belong to the source producer because the reference can apply species-specific time corrections. |
| Airborne | Mass carried by live particles. Pure transport, mixing, settling and reflecting boundaries preserve each species mass. |
| Wet | Airborne debit from scavenging, credited at the wet-deposition phase. Use the unadjusted transfer before output-grid decay corrections. |
| Dry | Airborne debit from dry deposition, credited once after transport. Use the post-primary-decay contribution from the coupled branch when decay is supported, before output-grid correction. |
| Decayed | Cumulative mass consumed by an admitted radioactive-decay operation. Zero in the v1 executable stable envelope. |
| Horizontal exit | Remaining airborne mass when a trajectory terminates at the model boundary, before the post-transport dry/decay pass. |
| Model exit | Remaining airborne mass on post-physics retirement for maximum age or minimum remaining mass. Age only, minimum mass only, and both predicates are mutually exclusive accounting reasons. |

`injected = airborne + wet + dry + decayed + horizontal exits + model exits`

Ordinary forward model-top placement and ground reflection are transport
actions. They do not debit airborne mass. Leaving an output grid does not
constitute a model exit. A stopped slot transfers its full remaining airborne
stock before deactivation, compaction or reuse. FLEXPART's
`terminate_particle` clears liveness and records an end time
(`particle_mod.f90:357–381`). The ledger requires a named credit at that
boundary so retained bytes in a dead slot cannot be counted as airborne or
credited on reuse. The pin sets `minmassfrac=0` (`par_mod.f90:178`). In the
stable profile a minimum-mass retirement carries zero residual mass. Any
positive residual caused by underflow or scale loss fails validation.

## Clock and operator order

In this table, *t* is the current synchronization boundary, and the outgoing
interval advances from *t* to *t + |LSYNCTIME|*. The terminal boundary executes
through sampling. Its closure has no outgoing interval.

| Order at *t* | Pinned source and accounting boundary |
| --- | --- |
| 1. Wet deposition | `timemanager_mod.f90:213–245` reads meteorology and calls wet deposition at nonzero model time if particles exist. `wetdepo_mod.f90:100–145` visits the already alive cohort. Existing particles are debited using the full absolute sampling interval passed by the time manager. Newly released particles do not participate in this call. |
| 2. Release and forward convection | `timemanager_mod.f90:291–370`, ordinary `releaseparticles(itime)` branch, then enabled forward convection. `initialise_mod.f90:249–281` assigns the particle species mass and initializes mass conversion. The release producer owns the actual credit. |
| 3. Deposited output adjustment and sampling | `timemanager_mod.f90:372–418` applies the scheduled deposited-field decay at `loutnext` before `output_conc`. The sample observes the post-wet, post-release state and effects from the previous interval. `output_mod.f90:500–516` gates this sample and gives the averaging endpoints half weight. The time manager exits at the final boundary before starting transport. |
| 4. Transport and horizontal exits | `timemanager_mod.f90:480–541` advances trajectories and terminates model-boundary exits before the post-transport physics pass. `advance_mod.f90:279–284,639–646,857–895` distinguishes horizontal stops from ordinary forward top placement and builds dry probability during transport before ground reflection. |
| 5. Dry removal and airborne decay | `timemanager_mod.f90:562–596` works on survivors. With dry deposition, `drydepo_mod.f90:324–350` uses a coupled split after discounting both branches by the same full-sync decay factor. Without dry deposition it applies that factor to airborne mass alone. |
| 6. Model retirement and closure | `timemanager_mod.f90:598–617,636–661` tests maximum remaining mass fraction and maximum age, then terminates. Record the remaining mass once, then finalize the ledger boundary before compaction or slot reuse. |

The three model-retirement reasons in the record are consecutive accounting
entries after the combined retirement decision. They do not prescribe three
separate sweeps or change the underlying Fortran predicates.

For incoming airborne mass *A*, dry removal probability *p* and one-interval
survival factor *d*, the dry branch gives dry credit *A p d*, surviving airborne
*A (1 − p) d* and primary decay credit *A (1 − d)* in real arithmetic. Wet uses
the unadjusted credit before airborne decay of the remaining cohort. The pin
accumulates unadjusted `wetdeposit` and already-discounted `drydeposit` per
particle and separately adjusts the deposition passed to output grids.
`wetdepo_mod.f90:145–174`, `drydepo_mod.f90:342–350` and
`unc_mod.f90:200–251` use different time references for those projections.
The time manager and wet routine also differ in the strictness of their
`loutnext` comparisons for `ldeltat`. #39 owns the decision and oracle proof
for aging deposited physical stock, the conversion to the output reference
time and attribution of the corresponding decay credit. The projection can
increase a gridded deposit to express it at an earlier reference time. It
must never increase the physical ledger or debit airborne again.

## Validation and GPU ownership

- Input, observed, and stored masses must be finite and nonnegative. A transfer
  cannot exceed available stock. Checkpoints and skipped operators preserve mass
  exactly. Missing phases, unknown exit reasons, inactive slot reads as live
  stock, and negative or invalid arithmetic are fatal. A zero-mass source must
  contribute zero mass.
- Per phase the audit compares the observed airborne change with the
  independently reported transfer, using the transfer as the relative
  denominator. Unresolvable transfers fail validation. Per boundary, each
  species and the total must close relative to actual cumulative injection.
  The ceiling is the issue's `1e-4`, with zero absolute tolerance and exact
  comparison for zero reference, complete debit or no-op. No operator may put
  the residual in a sink, clamp a negative result, or renormalize masses to
  attain closure.
- The device producer owns exactly one debit/credit pair per active particle,
  species and phase. Snapshot and credit must originate independently of the
  closure equation. Physical ledger and gridding consumers use the same transfer
  payload, including species, event time and location where applicable. Aliases
  such as probabilities or gridded deposits cannot supply a missing credit.
- The production owner must keep the particle stock and pending ledger credits
  on the same device through dependent passes and publish both under the same
  eligibility decision. Per-particle **status**, distinct from mass, identifies
  unknown retirements, nonfinite/negative lanes and accumulation failures.
  Reject failed runs at validation/output boundaries before exposing usable
  scientific artifacts. A canonical reducer must fix association or provide a
  bounded representation and deterministic arbitration, using step, phase,
  particle slot and species slot to report a reproducible first fault. A host
  readback belongs at explicit validation/output, per the [GPU contract](GPU_CONTRACT.md).
  The Rust audit operates at that boundary and cannot certify device execution.

## Production crosswalk and handoffs

| Owner | Required handoff |
| --- | --- |
| #34 | Emit one wet debit and unadjusted credit before injecting the new cohort. Obtain rates for the pre-release particle state from the canonical producer. Carry event coordinates/time separately to the spatial consumer. FLEXPART zeros rest mass at or below `tiny(0.0)` after crediting the wet amount. Positive residual lost there requires its own oracle-backed attribution or a fail-closed envelope. |
| #37 | Preserve dry probability formation during the transport substeps. The survivor's dry credit and spatial input share a producer. Ground reflection and settling motion do not create credits in addition to dry removal. |
| #39 | Implement and verify the coupled airborne/dry split and deposited-stock lifetime under the pinned output clock. Preserve separate physical and projected ledgers. Daughter ingrowth, chemical loss, non-kg forward unit routes and backward receptor attribution are outside v1. |
| #41 | Read the validated air stock at the scheduled sample boundary and consume the deposition events plus explicit output clock and coverage. Reconcile output against the corresponding ledger window and domain coverage. Keep top-level and nested gridding projections from double counting the same transfer. |

The [forward coordinator](../src/simulation/timeloop/forward/timestep.rs)
currently injects before the GPU block and advances through its inclusive last
call. The [operator block](../src/simulation/timeloop/forward/operators.rs)
applies post-transport dry, wet and then airborne-only decay. Its probability
buffers carry fractions. They cannot prove a mass credit.
[Output](../src/simulation/timeloop/forward/output.rs) is an explicit snapshot of
current particles. The resident transport guard supports the separately owned
backward top stop, while forward horizontal coordinates are clamped
([transport handoff](research/settling-stop-203.md#unsupported-envelope)).
A scientifically valid production ledger therefore needs a forward transport
exit event, prepare/commit coupling with the ledger, reordered deposition
handoffs, and an explicit sampling boundary. Forward horizontal-stop adoption
has no owner in the current transport handoff. Existing flows cannot yield a
passing v1 production ledger by reconstructing losses from probabilities or
grids. The resident ledger and production acceptance gate remain open.

## Reproduce

Run `cargo test --test mass_ledger` for injection-only, wet-only, dry-only,
exit-only and coupled two-species analytical records plus invalid inputs. Run
`python scripts/check_mass_ledger_reference.py --repository <FLEXPART repository>`
to check the frozen anchors directly against the Git object at the pin. The
source check establishes source identity, while the Rust tests establish the
analytical accounting behavior. Real-device production acceptance separately
requires operator-produced credits, per-species/total closure across timesteps,
and adapter and input evidence from the actual driver.
