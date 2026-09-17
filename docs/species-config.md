# Species configuration compatibility

The project-owned species schema is version 1. A species file may declare
`VERSION=1` (or `SPECIES_VERSION=1`) in its `&SPECIES` block or key/value
content. `SimulationConfig` and `SpeciesConfig` also carry this version when
serialized. Validation rejects version 0 and versions newer than 1 with an
explicit error; callers deserializing these Rust types must call `validate()`
before simulation.

Unversioned input is the legacy format. This includes FLEXPART 11.1
`&SPECIES_PARAMS` files and older project `&SPECIES` and key/value files.
Their existing aliases and units remain fixed: `PDECAY` is a half-life in
seconds, `PDRYVEL` is in cm/s, and `PDIA` is in metres. Project-owned
`dry_deposition_velocity` is in m/s, and `mean_diameter_um` is in micrometres.
New interpretations of a field must use a new schema version and an explicit
migration rather than changing how unversioned files are read.

Particle storage currently has four mass slots. Loading five or more species
fails before simulation with the actual count and supported maximum. Expanding
that limit requires a coordinated CPU/GPU particle-layout change.
