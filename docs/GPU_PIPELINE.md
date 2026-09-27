# GPU Calculation Pipeline Overview

> **Status:** architectural companion to [`GPU_CONTRACT.md`](GPU_CONTRACT.md).
>
> `GPU_CONTRACT.md` is the normative repository-wide contract. This document maps how data and calculations are expected to flow through the repository. If the two ever conflict, `GPU_CONTRACT.md` wins and both documents must be reconciled.

## Purpose

This document provides one end-to-end view of the calculative GPU architecture in FLEXPART-GPU. It is intentionally broader than any one implementation ticket.

It distinguishes:

- the currently implemented production path;
- the target canonical meteorology-to-physics path being established by #91, #87–#90, #76 and #77;
- host/device ownership and transfer boundaries;
- reusable GPU composition boundaries;
- explicit output/validation readback boundaries.

The pipeline is **not** a claim that pristine FLEXPART is one linear call stack. The pinned interpolation oracle documents multiple sibling call paths. Where a scientific operation has a required order, that order comes from the relevant oracle/issue contract rather than from this architecture picture.

## Architectural layers

| Layer | Primary responsibility | Default residency |
| --- | --- | --- |
| Provider ingestion | Decode provider-specific GRIB/NetCDF/file representations and normalize provider conventions | Host |
| Canonical meteorology | Immutable provider-independent `meteorology::Snapshot` and validated canonical metadata (#29) | Host before upload |
| Derived runtime geometry | Provider-independent pressure/height, AGL/ASL, terrain and W/interface geometry (#30) | Host before upload; reusable on device after upload |
| GPU meteorology resources | Persistent canonical field brackets, runtime geometry and related metadata required by GPU kernels | Device |
| Canonical GPU sampling | Horizontal, vertical, temporal and accumulated-field semantics (#87–#90), composed by #76 | Device |
| Production GPU physics | Existing and future GPU consumers migrated/integrated by #77 and later tickets | Device |
| Output / validation | Concentration output, diagnostics, oracle comparison and genuinely host-consumed state | Device until an explicit D2H boundary |

## Current implemented production path

As of 2026-09-27, the production time loop already demonstrates the repository's intended GPU composition style, but it still uses the pre-#76 meteorology interfaces in several places.

Current high-level flow:

```mermaid
flowchart TD
    subgraph HOST[Host / Rust orchestration]
        CFG[Config + release scheduling]
        MET[Current meteorology loading / bracket management]
        SURF[Surface-field preparation]
        REL[Particle release]
    end

    subgraph GPU[GPU / wgpu + WGSL]
        PB[(Persistent particle buffers)]
        WB[(Persistent wind bracket buffers / textures)]
        PBLB[(PBL buffers)]
        PBL[PBL diagnostics]
        ADV[Advection]
        TURB[Fused Hanna + Langevin\nproduction path]
        DRY[Dry deposition]
        WET[Wet deposition]
        DECAY[Decay when enabled]
        GRID[Concentration gridding]
    end

    REL -->|incremental H2D| PB
    MET -->|bracket change H2D| WB
    SURF -->|explicit H2D| PBL
    PBL --> PBLB
    WB --> ADV
    PB --> ADV
    ADV --> TURB
    PBLB --> TURB
    TURB --> DRY --> WET --> DECAY
    DECAY --> PB
    PB --> GRID
    GRID -->|explicit output D2H| OUT[Host output writer]
    PB -. optional diagnostics / synchronization .->|explicit D2H| HOSTSYNC[Host particle state]
```

Important existing architectural facts:

- `GpuContext` owns the common `wgpu::Device` and `wgpu::Queue`.
- Persistent wind, particle and PBL resources already exist.
- Production GPU work is composed with `encode_*` functions into caller-owned command encoders where available.
- Wind brackets are uploaded when the meteorological bracket changes rather than materializing a fresh host-side wind field for every particle step.
- The production path already avoids mandatory particle readback every timestep when host synchronization is disabled.
- Validation may split fused production stages into separate kernels without changing the production architecture.

This current path is evidence for the architecture; it is **not** the final canonical meteorology integration boundary.

## Target repository-wide pipeline

The target is a device-resident calculation loop in which provider concerns end at the canonical host boundary, canonical meteorology is uploaded explicitly, and GPU-capable consumers exchange GPU resources rather than host materializations.

```mermaid
flowchart TD
    subgraph HOST[Host boundary]
        RAW[Provider data / fixtures]
        DEC[Provider adapter / decoding\n#32 where applicable]
        CAN[Canonical meteorology Snapshot\n#29 immutable + validated]
        GEO[Derived runtime vertical geometry\n#30]
        REL[Release / config / scheduling]
    end

    subgraph DEVICE[Device-resident calculation domain]
        METRES[(Canonical meteorology GPU resources\nfield brackets + metadata + runtime geometry)]
        ACC[#90 accumulated-field\ninterval/reset transformation]
        RATE[(Derived interval amount/rate resources)]
        SAMPLER[#76 canonical 4D GPU sampling composition]
        H[#87 horizontal semantics]
        V[#88 vertical semantics]
        T[#89 instantaneous temporal semantics]
        SAMPLE[(Sampled canonical meteorology\nor inline/fused GPU values)]
        PB[(Persistent particle state)]
        CONSUMERS[#77 migrated production consumers]
        PHYS[GPU physics stages\nadvection / PBL-turbulence / convection / deposition / settling / decay / later physics]
        GRID[GPU output gridding / aggregation]
    end

    RAW --> DEC --> CAN
    CAN --> GEO
    CAN -->|explicit H2D on new/changed resources| METRES
    GEO -->|explicit H2D when geometry changes| METRES

    METRES --> ACC --> RATE
    METRES --> SAMPLER
    RATE --> SAMPLER
    H --> SAMPLER
    V --> SAMPLER
    T --> SAMPLER

    REL -->|incremental particle/config H2D| PB
    PB -->|sample coordinates / particle state| SAMPLER
    SAMPLER --> SAMPLE
    SAMPLE --> CONSUMERS --> PHYS --> PB

    PB --> GRID
    GRID -->|explicit D2H at output boundary| OUTPUT[Host output / persistence]
    SAMPLE -. validation only .->|explicit D2H| VALID[Oracle / diagnostic comparison]
```

### What this diagram means

1. **Provider-specific semantics stop before the canonical boundary.** Physics and GPU interpolation consume canonical fields and metadata, not GRIB ids, NetCDF names, provider units or provider sign conventions.
2. **The canonical Snapshot remains immutable.** #30 derives runtime geometry instead of mutating the canonical input into another coordinate system.
3. **Canonical meteorology and derived runtime geometry become reusable GPU resources.** They should be uploaded when the underlying source/bracket changes, not per particle query.
4. **#90 is a field-class preprocessing branch, not simply the fourth kernel in a universal serial chain.** Accumulated observations are transformed into explicit interval amounts/rates; #76 consumes the resulting rate semantics.
5. **#76 owns composition.** #87, #88 and #89 provide the horizontal, vertical and instantaneous temporal semantics; #90 provides accumulated-field interval/reset semantics. #76 determines how the relevant semantics are composed for each field/query without reimplementing them.
6. **A sampled result need not be materialized as a standalone buffer.** It may be a device buffer, a reusable GPU resource, or an inline/fused value path, provided validation/evidence remains possible and no hidden host round trip is introduced.
7. **#77 is the production-consumer migration boundary.** Existing physics consumers should consume the canonical GPU meteorology path rather than keeping independent interpolation implementations for the same fields.
8. **Particle state remains device-resident across timesteps by default.** The updated particle state feeds the next timestep's meteorological queries and physics.

## Scientific interpolation order versus architecture order

Do not read the target graph as one fixed function-call sequence for every field.

The pinned FLEXPART 11.1 interpolation contract explicitly records sibling production call paths rather than inventing one synthetic stack. For the relevant instantaneous 3-D wind sampling path, however, the frozen scientific primitive order is:

`horizontal interpolation -> vertical interpolation -> temporal interpolation`

That scientific order must be preserved where the owning issue/oracle requires it. Kernel fusion or encode-level composition may combine implementation stages, but may not change the resulting semantics.

Accumulated fields follow a different boundary:

`canonical accumulated observations -> #90 interval/reset transformation -> explicit amount/rate field -> #76 sampling semantics`

The accumulated-field transformation is therefore not interchangeable with ordinary #89 instantaneous temporal interpolation.

## Host/device transfer boundaries

### Expected H2D transfers

| Transfer | Expected cadence |
| --- | --- |
| Canonical meteorology field resources / brackets | On initial load and when the relevant meteorology resource/bracket changes |
| #30 derived runtime geometry | On initial load and when its source geometry changes |
| New particle releases | Incrementally when releases occur |
| Small per-step parameters/uniforms | Per timestep as required |
| Explicit externally supplied forcing that has not yet been migrated | Only while a documented migration boundary still owns it |

### Allowed D2H transfers

Device-to-host transfer is appropriate only for a real boundary such as:

- final or scheduled model output;
- oracle/scientific validation;
- diagnostics explicitly requested by the host;
- a genuinely host-owned downstream consumer;
- failure/debug evidence whose collection is intentionally enabled.

It is not an acceptable default between two GPU-capable calculation stages.

## Synchronization and command composition

The preferred production pattern is:

`prepare resources -> encode multiple dependent GPU stages -> submit -> wait/read back only at a real boundary`

Consequences:

- reusable stages should expose an `encode_*` path when they participate in larger GPU calculations;
- `dispatch_*` convenience APIs may submit/wait when standalone completion is part of their public contract;
- stage-to-stage host waits must not be introduced merely for implementation convenience;
- a `device.poll`/mapping wait belongs at a host-visible synchronization/readback boundary, not in an ordinary device-resident handoff;
- resource lifetime must span the consumers that need the data rather than forcing reconstruction or re-upload per stage.

## Resource lifetime model

The target resource lifetime is deliberately coarse-grained:

- **GPU context:** process/run lifetime;
- **particle buffers:** simulation lifetime, resized/replaced only when required;
- **canonical meteorology resources:** meteorological bracket/resource lifetime;
- **derived vertical geometry:** lifetime of the canonical geometry it was derived from;
- **per-field transformed accumulation/rate resources:** reusable across queries for the interval they represent;
- **per-step uniforms and query parameters:** timestep/dispatch lifetime;
- **readback staging resources:** explicit validation/output lifetime only.

#91 must audit whether existing shared buffer abstractions are sufficient for these lifetimes before introducing a new generic resource layer.

## Production consumer families

The canonical meteorology path ultimately needs to support the actual field consumers identified by the canonical meteorology contract. These include, as applicable:

- advection;
- PBL/turbulence;
- convection;
- wet deposition;
- dry deposition;
- gravitational settling;
- vertical/coordinate transforms and related diagnostics;
- later GPU physics that consumes canonical meteorological fields.

Not every consumer requires the same sampled fields or the same dimensionality. #77 should therefore migrate consumers by explicit field/consumer inventory rather than by forcing all physics through one oversized per-particle structure.

Processes that do not require meteorological sampling, such as a purely parameterized decay stage, still remain subject to the repository-wide GPU execution, composition, fallback and evidence rules.

## Current-to-target migration map

| Concern | Current repository behavior | Target boundary |
| --- | --- | --- |
| Meteorology input | Existing time loop still uses legacy/current wind/surface structures in production paths | Provider-independent #29 Snapshot + #30 runtime geometry as source contract |
| Temporal wind use | Dual wind brackets + GPU-side alpha already demonstrate device-side time sampling | Generalized #89 semantics through #76 for canonical instantaneous fields |
| Horizontal/vertical sampling | Existing advection/interpolation shaders contain specialized sampling paths | Canonical #87/#88 semantics reused through #76 |
| Accumulated precipitation | Canonical CPU semantics exist from #75 | #90 GPU transformation, then #76 consumes explicit rates/amounts |
| Physics consumers | Several kernels still own specialized field access/forcing paths | #77 migrates applicable consumers to canonical GPU meteorology access |
| Particle state | Persistent GPU buffers already exist | Preserve device residency across production timesteps |
| Output | GPU gridding + explicit host output is already present | Preserve as an intentional D2H boundary |

## Issue ownership map

| Issue | Pipeline responsibility |
| --- | --- |
| #29 | Canonical provider-independent meteorology schema and boundary validation |
| #30 | Derived runtime vertical geometry and AGL/ASL semantics |
| #71 | Pinned FLEXPART interpolation oracle evidence |
| #80 | Production W/interface vertical semantics required by #88 |
| #91 | Repository-wide GPU execution/ownership/composition/validation contract and shared infrastructure audit |
| #87 | GPU horizontal interpolation semantics |
| #88 | GPU vertical sampling semantics |
| #89 | GPU instantaneous temporal interpolation semantics |
| #90 | GPU accumulated-field interval/reset transformation |
| #76 | Canonical 4D meteorology sampling composition |
| #77 | Migration of production meteorology consumers to the canonical sampling path |
| Later GPU tickets | Must preserve the same execution, residency, transfer, fallback and evidence rules |

## Decisions deliberately not frozen here

This overview does **not** pre-decide algorithm-specific details owned elsewhere, including:

- the exact concrete Rust type used for future GPU meteorology resources;
- whether a scientifically equivalent group of stages is implemented as separate kernels or fused;
- issue-specific numerical tolerances;
- a universal field layout where different scientific fields require different dimensionality/staggering;
- new physics models;
- provider-specific decoding or variable mappings;
- an artificial single call stack where the pinned FLEXPART source has sibling production paths.

Those decisions remain with the owning scientific issue, except where #91 establishes a repository-wide infrastructure rule.

## Review rule for future calculative GPU tickets

A GPU calculation change should be rejected or sent back for explicit architectural review if it unnecessarily:

1. creates a second GPU runtime/device/queue ownership model;
2. downloads an intermediate result only to upload it again to another GPU stage;
3. hides a D2H transfer inside an ordinary calculation API;
4. forces a standalone submit/wait between stages that can be encoded together;
5. introduces a silent CPU replacement path for a GPU-supported production calculation;
6. duplicates canonical meteorology sampling semantics instead of using the common path where applicable;
7. makes downstream GPU composition impossible without host materialization.

## Related documentation

- [`GPU_CONTRACT.md`](GPU_CONTRACT.md) — normative repository-wide GPU rules.
- [`architecture.md`](architecture.md) — broader repository architecture and current GPU implementation.
- [`science/simulation-flow.md`](science/simulation-flow.md) — detailed current time-loop execution flow.
- [`meteorology-contract.md`](meteorology-contract.md) — canonical meteorology schema and consumer field matrix.
- [`interpolation-contract.md`](interpolation-contract.md) — pinned FLEXPART 11.1 interpolation semantics and call-edge evidence.
- [`accumulation-contract.md`](accumulation-contract.md) — accumulated-field interval/reset semantics.
