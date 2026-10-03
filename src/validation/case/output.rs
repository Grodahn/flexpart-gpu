//! Output direction, averaging/sampling timing and comparison-grid contract.

use super::{HorizontalCoordRef, ValidationCaseError, ValidationCaseManifest, VerticalRef};
use serde::{Deserialize, Serialize};

/// Explicit concentration/comparison output grid.
///
/// This is deliberately separate from the meteorological/candidate domain:
/// ETEX uses a 65x41x16 meteorological grid but a 64x40x5 concentration
/// output grid. Every schema-v2 case declares this block explicitly; no
/// synthetic fallback to the meteorological domain is permitted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputGridSpec {
    pub nx: u32,
    pub ny: u32,
    pub nz: u32,
    pub dx_deg: f32,
    pub dy_deg: f32,
    pub xlon0_deg: f32,
    pub ylat0_deg: f32,
    pub horizontal_ref: HorizontalCoordRef,
    pub heights_m: Vec<f32>,
    pub heights_ref: VerticalRef,
}

/// Simulation direction, i.e. the FLEXPART `LDIRECT` semantic
/// (`readoptions_mod.f90`): `ldirect` contains the direction of time,
/// 1 for forward, -1 for backward.
///
/// Required and typed so a document can never fall back to an implicit
/// direction (Issue #51 / #57): the raw numeric key stays on the generated
/// namelist, while the manifest carries the canonical semantic value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SimulationDirection {
    /// Forward simulation in time (FLEXPART `LDIRECT = 1`).
    Forward,
    /// Backward simulation in time (FLEXPART `LDIRECT = -1`).
    Backward,
}

impl SimulationDirection {
    /// FLEXPART COMMAND `LDIRECT` value this direction maps to.
    #[must_use]
    pub const fn flexpart_ldirect(self) -> i32 {
        match self {
            SimulationDirection::Forward => 1,
            SimulationDirection::Backward => -1,
        }
    }
}

/// Scientific quantity represented by each produced output field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputQuantity {
    /// Time-averaged mass concentration [kg/m3] over the averaging window.
    TimeAveragedMassConcentrationKgM3,
}

/// Output timing and scientific semantics (Issue #51 / #57).
///
/// Maps one-to-one onto the FLEXPART COMMAND keys `LOUTSTEP` / `LOUTAVER` /
/// `LOUTSAMPLE` (`readoptions_mod.f90`): an output field is written every
/// `interval_s` seconds, its values averaging particle samples taken every
/// `sampling_interval_s` seconds across the `averaging_window_s` window.
/// The FLEXPART binary header writes the triplet verbatim
/// (`binary_output_mod.f90`). No workflow default cushions a missing field.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputSpec {
    /// Output interval (FLEXPART `LOUTSTEP`) [s].
    pub interval_s: u32,
    /// Averaging window (FLEXPART `LOUTAVER`) [s].
    pub averaging_window_s: u32,
    /// Sampling interval (FLEXPART `LOUTSAMPLE`) [s].
    pub sampling_interval_s: u32,
    /// Scientific quantity each output field represents.
    pub quantity: OutputQuantity,
}

impl ValidationCaseManifest {
    pub(super) fn validate_output_grid(&self) -> Result<(), ValidationCaseError> {
        let Some(grid) = &self.output_grid else {
            return Err(ValidationCaseError::MissingField {
                field: "output_grid",
            });
        };
        if grid.heights_ref != VerticalRef::Agl {
            return Err(ValidationCaseError::AmbiguousField {
                field: "output_grid.heights_ref",
                message: "schema v2 currently supports only AGL output heights; ASL conversion semantics are not implemented".to_string(),
            });
        }

        if grid.nx == 0 || grid.ny == 0 || grid.nz == 0 {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: "output_grid nx/ny/nz must be > 0".to_string(),
            });
        }
        for (field, value) in [
            ("output_grid.dx_deg", grid.dx_deg),
            ("output_grid.dy_deg", grid.dy_deg),
        ] {
            if !value.is_finite() || value <= 0.0 {
                return Err(ValidationCaseError::InvalidPhysicsSwitches {
                    message: format!("{field} must be finite and > 0, got {value}"),
                });
            }
        }
        if !grid.xlon0_deg.is_finite() || !(-180.0..=360.0).contains(&grid.xlon0_deg) {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!(
                    "output_grid.xlon0_deg must be finite and in [-180, 360], got {}",
                    grid.xlon0_deg
                ),
            });
        }
        if !grid.ylat0_deg.is_finite() || !(-90.0..=90.0).contains(&grid.ylat0_deg) {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!(
                    "output_grid.ylat0_deg must be finite and in [-90, 90], got {}",
                    grid.ylat0_deg
                ),
            });
        }
        if grid.heights_m.len() != grid.nz as usize {
            return Err(ValidationCaseError::AmbiguousField {
                field: "output_grid.heights_m",
                message: format!(
                    "heights_m length ({}) must equal output_grid.nz ({})",
                    grid.heights_m.len(),
                    grid.nz
                ),
            });
        }
        if grid.heights_m.iter().any(|h| !h.is_finite() || *h < 0.0) {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: "output_grid.heights_m must be finite and >= 0".to_string(),
            });
        }
        if grid.heights_m.windows(2).any(|w| w[1] <= w[0]) {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: "output_grid.heights_m must be strictly increasing".to_string(),
            });
        }
        Ok(())
    }

    pub(super) fn validate_output(&self) -> Result<(), ValidationCaseError> {
        if self.simulation_direction != SimulationDirection::Forward {
            return Err(ValidationCaseError::AmbiguousField {
                field: "simulation_direction",
                message: "backward is a valid FLEXPART mode but is deliberately unsupported by schema v2: the current OutputQuantity models forward time-averaged mass concentration, while backward IOUT=1 uses source-receptor/residence-time semantics".to_string(),
            });
        }

        let output = &self.output;
        for (name, magnitude) in [
            ("output.interval_s", output.interval_s),
            ("output.averaging_window_s", output.averaging_window_s),
            ("output.sampling_interval_s", output.sampling_interval_s),
        ] {
            if magnitude == 0 {
                return Err(ValidationCaseError::InvalidPhysicsSwitches {
                    message: format!("{name} must be > 0, got {magnitude}"),
                });
            }
        }
        if output.sampling_interval_s > output.averaging_window_s {
            return Err(ValidationCaseError::AmbiguousField {
                field: "output.sampling_interval_s",
                message: format!(
                    "sampling interval {} s must not exceed averaging window {} s (FLEXPART LOUTSAMPLE <= LOUTAVER)",
                    output.sampling_interval_s, output.averaging_window_s
                ),
            });
        }
        if output.averaging_window_s > output.interval_s {
            return Err(ValidationCaseError::AmbiguousField {
                field: "output.averaging_window_s",
                message: format!(
                    "averaging window {} s must not exceed output interval {} s (FLEXPART LOUTAVER <= LOUTSTEP)",
                    output.averaging_window_s, output.interval_s
                ),
            });
        }

        let sync =
            self.oracle_command_overrides
                .lsynctime_s
                .ok_or(ValidationCaseError::MissingField {
                    field: "oracle_command_overrides.lsynctime_s",
                })?;
        if sync == 0 {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: "oracle_command_overrides.lsynctime_s must be > 0".to_string(),
            });
        }
        for (name, magnitude) in [
            ("output.interval_s", output.interval_s),
            ("output.averaging_window_s", output.averaging_window_s),
            ("output.sampling_interval_s", output.sampling_interval_s),
        ] {
            if magnitude % sync != 0 {
                return Err(ValidationCaseError::AmbiguousField {
                    field: name,
                    message: format!(
                        "{name}={magnitude} s must be a multiple of the declared FLEXPART LSYNCTIME={sync} s"
                    ),
                });
            }
        }
        if output.averaging_window_s < 2 * sync {
            return Err(ValidationCaseError::AmbiguousField {
                field: "output.averaging_window_s",
                message: format!(
                    "averaging window {} s must be at least 2*LSYNCTIME={} s",
                    output.averaging_window_s,
                    2 * sync
                ),
            });
        }
        if output.interval_s < 2 * sync {
            return Err(ValidationCaseError::AmbiguousField {
                field: "output.interval_s",
                message: format!(
                    "output interval {} s must be at least 2*LSYNCTIME={} s",
                    output.interval_s,
                    2 * sync
                ),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{
        load_checked_in_case, load_validation_case_schema, make_minimal_manifest,
        minimal_manifest_json, parse_json_value, validate_json_schema_subset, ALL_CHECKED_IN_CASES,
    };

    use super::super::*;
    use std::path::Path;

    #[test]
    fn simulation_direction_round_trips_and_maps_to_flexpart_ldirect() {
        let forward: SimulationDirection =
            serde_json::from_str("\"forward\"").expect("forward variant");
        let backward: SimulationDirection =
            serde_json::from_str("\"backward\"").expect("backward variant");
        assert_eq!(forward, SimulationDirection::Forward);
        assert_eq!(
            serde_json::to_string(&forward).expect("serialize forward"),
            "\"forward\""
        );
        assert_eq!(
            serde_json::to_string(&backward).expect("serialize backward"),
            "\"backward\""
        );
        assert!(
            serde_json::from_str::<SimulationDirection>("\"Forward\"").is_err(),
            "unknown variant must not silently coerce"
        );
        assert_eq!(forward.flexpart_ldirect(), 1);
        assert_eq!(backward.flexpart_ldirect(), -1);
    }

    #[test]
    fn missing_simulation_direction_is_rejected() {
        let mut raw = minimal_manifest_json();
        raw.as_object_mut()
            .expect("manifest object")
            .remove("simulation_direction");
        let err = parse_json_value(&raw).expect_err("missing direction fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("simulation_direction"),
            "error must name the missing field: {rendered}"
        );
    }

    #[test]
    fn missing_output_spec_is_rejected() {
        let mut raw = minimal_manifest_json();
        raw.as_object_mut()
            .expect("manifest object")
            .remove("output");
        let err = parse_json_value(&raw).expect_err("missing output fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("output"),
            "error must name the missing field: {rendered}"
        );
    }

    #[test]
    fn missing_individual_output_timing_field_is_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.output.interval_s = 0;
        let err = manifest.validate().expect_err("zero interval fails");
        assert!(matches!(
            err,
            ValidationCaseError::InvalidPhysicsSwitches { .. }
        ));

        let mut raw = minimal_manifest_json();
        raw["output"]
            .as_object_mut()
            .expect("output object")
            .remove("interval_s");
        let err = parse_json_value(&raw).expect_err("missing interval_s fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("interval_s"),
            "error must name the missing key: {rendered}"
        );
    }

    #[test]
    fn zero_output_timing_rejected() {
        for (field, magnitude) in [
            ("interval_s", 0_u32),
            ("averaging_window_s", 0_u32),
            ("sampling_interval_s", 0_u32),
        ] {
            let mut manifest = make_minimal_manifest();
            match field {
                "interval_s" => manifest.output.interval_s = magnitude,
                "averaging_window_s" => manifest.output.averaging_window_s = magnitude,
                "sampling_interval_s" => manifest.output.sampling_interval_s = magnitude,
                _ => unreachable!(),
            }
            let err = manifest.validate().expect_err("zero timing fails");
            assert!(
                err.to_string().contains(field),
                "error must name {field}: {err}"
            );
        }
    }

    #[test]
    fn averaging_window_exceeding_output_interval_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.output.averaging_window_s = 3600;
        manifest.output.interval_s = 1800;
        let err = manifest.validate().expect_err("average > output fails");
        assert!(
            matches!(
                err,
                ValidationCaseError::AmbiguousField {
                    field: "output.averaging_window_s",
                    ..
                }
            ),
            "unexpected: {err}"
        );
    }

    #[test]
    fn backward_direction_is_rejected_until_output_semantics_are_modeled() {
        let mut manifest = make_minimal_manifest();
        manifest.simulation_direction = SimulationDirection::Backward;
        let err = manifest
            .validate()
            .expect_err("backward must fail closed in schema v2");
        assert!(matches!(
            err,
            ValidationCaseError::AmbiguousField {
                field: "simulation_direction",
                ..
            }
        ));
        assert!(err.to_string().contains("source-receptor"));
    }

    #[test]
    fn output_timings_must_match_declared_sync_interval() {
        let mut manifest = make_minimal_manifest();
        manifest.output.sampling_interval_s = 301;
        let err = manifest
            .validate()
            .expect_err("non-multiple sample must fail");
        assert!(err.to_string().contains("LSYNCTIME"));

        let mut manifest = make_minimal_manifest();
        manifest.output.interval_s = manifest
            .oracle_command_overrides
            .lsynctime_s
            .expect("lsynctime");
        manifest.output.averaging_window_s = manifest
            .oracle_command_overrides
            .lsynctime_s
            .expect("lsynctime");
        manifest.output.sampling_interval_s = manifest
            .oracle_command_overrides
            .lsynctime_s
            .expect("lsynctime");
        let err = manifest
            .validate()
            .expect_err("interval/average below 2*sync must fail");
        assert!(err.to_string().contains("2*LSYNCTIME"));
    }

    #[test]
    fn migrated_cases_keep_their_output_direction_semantics() {
        // Synthetic cases run forward with LOUTSTEP=1800 / LOUTAVER=1800 /
        // LOUTSAMPLE=300; ETEX-MINI-013 uses the real ETEX window
        // (LOUTSTEP=10800 / LOUTAVER=10800 / LOUTSAMPLE=900).
        for case_id in ALL_CHECKED_IN_CASES {
            let manifest = load_checked_in_case(case_id);
            assert_eq!(
                manifest.simulation_direction,
                SimulationDirection::Forward,
                "{case_id} direction changed"
            );
            assert_eq!(
                manifest.simulation_direction.flexpart_ldirect(),
                1,
                "{case_id} ldirect mapping"
            );
            if *case_id == "ETEX-MINI-013" {
                assert_eq!(manifest.output.interval_s, 10800, "{case_id}");
                assert_eq!(manifest.output.averaging_window_s, 10800, "{case_id}");
                assert_eq!(manifest.output.sampling_interval_s, 900, "{case_id}");
            } else {
                assert_eq!(manifest.output.interval_s, 1800, "{case_id}");
                assert_eq!(manifest.output.averaging_window_s, 1800, "{case_id}");
                assert_eq!(manifest.output.sampling_interval_s, 300, "{case_id}");
            }
            assert_eq!(
                manifest.output.quantity,
                OutputQuantity::TimeAveragedMassConcentrationKgM3,
                "{case_id} output quantity"
            );
        }
    }

    #[test]
    fn every_case_requires_explicit_output_grid() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("corpus")
            .join("cases")
            .join("WIND-UNI-002.json");
        let text = std::fs::read_to_string(&path).expect("read synthetic case");
        let mut raw: serde_json::Value =
            serde_json::from_str(&text).expect("parse synthetic case JSON");
        raw.as_object_mut()
            .expect("case object")
            .remove("output_grid");
        let err = parse_json_value(&raw).expect_err("case without output_grid must fail");
        assert!(err.to_string().contains("output_grid"), "unexpected: {err}");
    }

    #[test]
    fn output_grid_null_is_rejected_by_schema_and_rust() {
        let schema = load_validation_case_schema();
        let mut raw = minimal_manifest_json();
        raw["output_grid"] = serde_json::Value::Null;

        let schema_err = validate_json_schema_subset(&schema, &schema, &raw, "$")
            .expect_err("JSON Schema must reject output_grid=null");
        assert!(
            schema_err.contains("output_grid") || schema_err.contains("object"),
            "unexpected schema error: {schema_err}"
        );

        let err = parse_json_value(&raw).expect_err("Rust must reject output_grid=null");
        assert!(
            err.to_string().contains("output_grid"),
            "unexpected Rust error: {err}"
        );
    }
}
