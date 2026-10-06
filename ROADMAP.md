# Roadmap and Project Status

This document describes the path from the current experimental implementation to
a scientifically validated, operationally usable `flexpart-gpu` release.

It is intentionally conservative: code existing in the repository does not count
as "done" for scientific parity until the relevant production path has passed its
declared validation contract.

## Definition of production readiness

For this project, production readiness means more than successful GPU execution.

A production-ready forward-dispersion worker must have:

1. explicit and versioned source/release semantics;
2. operational meteorology ingestion into the canonical representation;
3. device-resident production transport using the canonical GPU meteorology path;
4. validated turbulence, convection, deposition, settling, decay, and mass
   accounting;
5. conservative, species-resolved concentration/deposition outputs;
6. reproducible comparison against pinned FLEXPART 11.1 inputs and outputs;
7. pre-registered ETEX-I and radionuclide validation evidence;
8. characterized determinism, precision, performance, hardware support, and
   memory limits;
9. fail-closed behavior for unsupported inputs, missing evidence, and resource
   limits.

## Milestone 1 — Validation and GPU foundations

**Status: substantially established**

Completed foundations include:

- species/nuclide GPU model (#10);
- canonical meteorology representation and vertical geometry (#29, #30);
- pinned FLEXPART interpolation/oracle fixtures (#71, #80);
- horizontal, vertical, temporal, and accumulated-field interpolation semantics
  (#72–#75);
- GPU ports of canonical meteorology sampling (#87–#90);
- repository-wide GPU execution contract (#91);
- oracle determinism, stochastic identity, versioned case manifests,
  candidate/oracle input-equivalence, and validation provenance (#49–#53).

These foundations are intended to be reused rather than reimplemented by later
physics and production work.

## Milestone 2 — Canonical production dataflow

**Status: in progress**

Primary work:

- compose canonical device-resident meteorology sampling (#76);
- migrate production consumers for advection, PBL/turbulence, deposition, and
  convection (#112–#115 under #77);
- remove transitional CPU meteorology paths after migration (#117);
- operational real-file meteorology decoding and ingestion (#32);
- canonical release/source-term contract and injection (#26–#28);
- stable logical particle/RNG identity (#168);
- cohort-independent source-mass partitioning (#169).

Exit condition: a supported release and real meteorology can enter one canonical
production GPU path without hidden alternate scientific implementations.

## Milestone 3 — Full physics and scientific outputs

**Status: active, not yet closed**

Primary work:

- horizontal and vertical/PBL transport parity (#7, #8, #62–#64);
- Emanuel convection (#23–#25 under #9);
- wet deposition (#33, #34);
- settling and dry deposition (#35–#37);
- authoritative mass ledger and radioactive decay (#38, #39);
- scientific output schema, additive accumulation/overflow contract, production
  gridding, and serialization (#40, #170, #41, #42).

Exit condition: source mass can be followed through airborne, deposited, decayed,
and exited reservoirs into scientifically defined output fields with closed mass
budgets.

## Milestone 4 — Scientific release validation

**Status: foundations available; final evidence pending**

Reusable validation infrastructure continues through #54–#61.

Final release validation is tracked by #17:

- pre-register cases, thresholds, and verdict requirements (#43);
- ETEX-I model-vs-model and model-vs-observation validation (#44);
- radionuclide validation and supported scientific envelope (#45).

Exit condition: the supported model envelope is stated explicitly and backed by
reproducible, attributable evidence rather than isolated kernel tests.

## Milestone 5 — Production performance and scaling

**Status: preliminary benchmarks exist; production envelope pending**

Existing replicated synthetic campaigns provide useful engineering baselines,
including 1M and 10M GPU-vs-Fortran measurements. They are not substitutes for
the final full-physics production workload.

Primary work:

- reproducible real-GPU full-physics benchmark harness (#46);
- determinism, precision, scaling, and hardware envelope (#47);
- canonical 48-hour Europe reference workload (#116);
- measured high-particle optimization program (#154);
- VRAM-budgeted particle cohort execution (#163) after logical identity,
  mass-partition, and accumulation contracts (#168–#170).

The 48-hour/15-minute workload in #116 is a benchmark fixture, not a hard-coded
simulation limitation.

Exit condition: supported hardware and workload limits are explicit, large
logical particle populations can be processed without coupling scientific meaning
to single-GPU VRAM, and performance claims are reproducible.

## Principles that apply across all milestones

- FLEXPART 11.1 remains the pinned normative model oracle where parity is claimed.
- Scientific contracts precede performance optimization.
- Production scientific arithmetic is GPU-first where technically practical.
- No silent CPU fallback may satisfy a GPU production acceptance criterion.
- Unsupported or unverifiable configurations fail closed.
- AI-assisted implementation does not lower review, provenance, or validation
  requirements.
- External research that materially inspires the design is attributed in the
  relevant issue/PR.

## Navigation

- Main scientific/production epic: #4
- Reusable validation pipeline: #48
- Production meteorology: #12 / #31 / #77
- Source terms: #11
- Scientific outputs: #16
- Release validation: #17
- Performance/scaling: #18
- High-particle optimization: #154

For detailed implementation contracts, dependencies, and acceptance criteria,
follow the linked GitHub issues. The issue tracker is authoritative when this
high-level roadmap and an issue differ.
