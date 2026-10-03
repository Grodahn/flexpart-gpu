//! Pinned oracle COMMAND overrides and turbulence/integration formulation checks.

use super::{ValidationCaseError, ValidationCaseManifest};
use serde::{Deserialize, Serialize};

/// Oracle turbulence/integration formulation selection (Issue #67).
///
/// FLEXPART derives two coupled behaviours from the COMMAND `ctl` value
/// (pinned oracle, reference/flexpart-11.1.json at commit c70586c):
///
/// - the dispersion method (`readoptions_mod.f90:786-795`): `ctl > 0` selects
///   the adaptive particle-timestep method (`method=1`, `mintime=minstep`);
///   `ctl <= 0` selects the fixed-timestep method (`method=0`,
///   `mintime=lsynctime`);
/// - the Markov-chain formulation (`readoptions_mod.f90:626,645-650`):
///   `ctl >= 0.1` selects the w/sigw formulation (`turbswitch=.true.`);
///   `ctl < 0.1` silently selects the w formulation and forces `ifine=1`.
///
/// Schema v2 supports the two formulations that are actually present in the
/// checked-in validation corpus. The synthetic corpus uses adaptive w/sigw;
/// ETEX-MINI-013 preserves its historical fixed-timestep / w formulation.
/// The typed value is authoritative and must agree with `ctl`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OracleTurbulenceFormulation {
    /// Adaptive integration (`method=1`, readoptions_mod.f90:786-795) with the
    /// w/sigw Markov formulation (`turbswitch=.true.`, readoptions_mod.f90:645-650).
    #[serde(rename = "adaptive_w_sigw")]
    AdaptiveWSigmaW,
    /// Fixed particle timestep (`method=0`, `mintime=lsynctime`) with the
    /// w Markov formulation selected by CTL < 0.1. FLEXPART forces effective
    /// IFINE=1 in this formulation even if the raw COMMAND contains another
    /// IFINE value; ETEX-MINI-013 historically contains IFINE=4.
    #[serde(rename = "fixed_sync_w")]
    FixedSyncW,
}

impl std::fmt::Display for OracleTurbulenceFormulation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            OracleTurbulenceFormulation::AdaptiveWSigmaW => "adaptive_w_sigw",
            OracleTurbulenceFormulation::FixedSyncW => "fixed_sync_w",
        })
    }
}

/// `ctl` threshold of the pinned oracle's w/sigw Markov formulation
/// (`turbswitch=.true.`, readoptions_mod.f90:645-650) and lower bound of the
/// adaptive dispersion method's valid `ctl` range. Single documented constant
/// shared with the Python generator contract (step 6 of Issue #67).
pub const CTL_W_SIGW_FORMULATION_THRESHOLD: f32 = 0.1;

/// Oracle command overrides (namelist values).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OracleCommandOverrides {
    /// Declared turbulence/integration formulation (required, never defaulted
    /// in a document; see `OracleTurbulenceFormulation`).
    pub turbulence_formulation: OracleTurbulenceFormulation,
    /// Turbulence flag (0/1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lturbulence: Option<u8>,
    /// Convection flag (0/1).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lconvection: Option<u8>,
    /// CTL parameter (Hanna turbulence scaling).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ctl: Option<f32>,
    /// IFINE value written to COMMAND. In fixed_sync_w FLEXPART forces the
    /// effective value to 1 internally (readoptions_mod.f90:645-650).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ifine: Option<u32>,
    /// FLEXPART synchronisation interval (COMMAND LSYNCTIME) [s].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lsynctime_s: Option<u32>,
}

impl ValidationCaseManifest {
    pub(super) fn validate_oracle_overrides(&self) -> Result<(), ValidationCaseError> {
        let overrides = &self.oracle_command_overrides;
        let flag = |name: &'static str, value: Option<u8>| -> Result<u8, ValidationCaseError> {
            let value = value.ok_or(ValidationCaseError::MissingField { field: name })?;
            if value != 0 && value != 1 {
                return Err(ValidationCaseError::InvalidPhysicsSwitches {
                    message: format!("{name} must be exactly 0 or 1, got {value}"),
                });
            }
            Ok(value)
        };
        let lturbulence = flag(
            "oracle_command_overrides.lturbulence",
            overrides.lturbulence,
        )?;
        let lconvection = flag(
            "oracle_command_overrides.lconvection",
            overrides.lconvection,
        )?;
        let ctl = overrides.ctl.ok_or(ValidationCaseError::MissingField {
            field: "oracle_command_overrides.ctl",
        })?;
        Self::validate_oracle_ctl_formulation(ctl, overrides.turbulence_formulation)?;
        let ifine = overrides.ifine.ok_or(ValidationCaseError::MissingField {
            field: "oracle_command_overrides.ifine",
        })?;
        let lsynctime_s = overrides
            .lsynctime_s
            .ok_or(ValidationCaseError::MissingField {
                field: "oracle_command_overrides.lsynctime_s",
            })?;
        if lsynctime_s == 0 {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: "oracle_command_overrides.lsynctime_s must be > 0".to_string(),
            });
        }
        if !(1..=10).contains(&ifine) {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!(
                    "oracle_command_overrides.ifine must be in 1..=10 (the oracle silently \
                     clamps IFINE to >= 1 via max(ifine,1), readoptions_mod.f90:624; the \
                     corpus contract caps the vertical sub-stepping at 10), got {ifine}"
                ),
            });
        }
        self.validate_oracle_physics_agreement(lturbulence, lconvection)?;
        Ok(())
    }

    /// Enforces the Issue #67 `ctl` contract for the pinned oracle
    /// (reference/flexpart-11.1.json):
    ///
    /// - `ctl` must be finite;
    /// - `ctl = 0` is a division by zero: the oracle computes `ctl = 1./ctl`
    ///   unconditionally (`readoptions_mod.f90:653`) and sizes particle steps
    ///   from it (`advance_mod.f90:557-568`);
    /// - `ctl < 0` selects the fixed-timestep dispersion mode (`method=0`,
    ///   `mintime=lsynctime`) and the w formulation; this is represented
    ///   explicitly as `fixed_sync_w` for ETEX-MINI-013;
    /// - `0 < ctl < CTL_W_SIGW_FORMULATION_THRESHOLD` selects adaptive timing
    ///   but silently switches the Markov chain to w and forces `ifine=1`;
    ///   that mixed mode is not present in the corpus and remains unsupported.
    fn validate_oracle_ctl_formulation(
        ctl: f32,
        formulation: OracleTurbulenceFormulation,
    ) -> Result<(), ValidationCaseError> {
        if !ctl.is_finite() {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!("oracle_command_overrides.ctl must be finite, got {ctl}"),
            });
        }
        if ctl == 0.0 {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!(
                    "oracle_command_overrides.ctl must be non-zero: the oracle computes \
                     ctl = 1./ctl unconditionally (readoptions_mod.f90:653) and sizes \
                     particle time steps from it (advance_mod.f90:557-568); CTL=0 is a \
                     division by zero producing a divergent step, got {ctl}"
                ),
            });
        }

        match formulation {
            OracleTurbulenceFormulation::AdaptiveWSigmaW => {
                if ctl < CTL_W_SIGW_FORMULATION_THRESHOLD {
                    return Err(ValidationCaseError::InvalidPhysicsSwitches {
                        message: format!(
                            "oracle_command_overrides.ctl={ctl} contradicts \
                             turbulence_formulation={formulation}: adaptive_w_sigw requires \
                             CTL >= {CTL_W_SIGW_FORMULATION_THRESHOLD}; CTL <= 0 selects \
                             fixed-timestep method=0/mintime=lsynctime and values below the \
                             threshold select the w formulation and force effective IFINE=1 \
                             (readoptions_mod.f90:645-650,786-795)"
                        ),
                    });
                }
            }
            OracleTurbulenceFormulation::FixedSyncW => {
                if ctl >= 0.0 {
                    return Err(ValidationCaseError::InvalidPhysicsSwitches {
                        message: format!(
                            "oracle_command_overrides.ctl={ctl} contradicts \
                             turbulence_formulation={formulation}: fixed_sync_w requires \
                             CTL < 0 so FLEXPART selects method=0 with mintime=lsynctime; \
                             CTL < 0.1 also selects the w formulation and forces effective \
                             IFINE=1 (readoptions_mod.f90:645-650,786-795)"
                        ),
                    });
                }
            }
        }
        Ok(())
    }

    /// Cross-checks that oracle COMMAND switches agree with the declared
    /// `physics_switches`, so a case cannot silently run different physics than
    /// it claims. Departures from Fortran module state are rejected as
    /// `InvalidPhysicsSwitches`.
    fn validate_oracle_physics_agreement(
        &self,
        lturbulence: u8,
        lconvection: u8,
    ) -> Result<(), ValidationCaseError> {
        if self.physics_switches.turbulence != (lturbulence == 1) {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!(
                    "physics_switches.turbulence={} conflicts with oracle lturbulence={lturbulence}",
                    self.physics_switches.turbulence
                ),
            });
        }
        if self.physics_switches.convection != (lconvection == 1) {
            return Err(ValidationCaseError::InvalidPhysicsSwitches {
                message: format!(
                    "physics_switches.convection={} conflicts with oracle lconvection={lconvection}",
                    self.physics_switches.convection
                ),
            });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{
        load_validation_case_schema, make_minimal_manifest, minimal_manifest_json,
        parse_json_value, validate_json_schema_subset,
    };

    use super::super::*;
    use std::path::Path;

    #[test]
    fn adv_ana_001_oracle_overrides_disable_turbulence_and_convection() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("corpus")
            .join("cases")
            .join("ADV-ANA-001.json");
        let manifest = ValidationCaseManifest::load_from_file(&path).expect("load ADV-ANA-001");
        assert_eq!(manifest.oracle_command_overrides.lturbulence, Some(0));
        assert_eq!(manifest.oracle_command_overrides.lconvection, Some(0));
        manifest
            .validate()
            .expect("ADV-ANA-001 overrides stay valid");
    }

    #[test]
    fn wind_uni_002_oracle_overrides_match_manifest() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("corpus")
            .join("cases")
            .join("WIND-UNI-002.json");
        let manifest = ValidationCaseManifest::load_from_file(&path).expect("load WIND-UNI-002");
        assert_eq!(manifest.oracle_command_overrides.lturbulence, Some(1));
        assert_eq!(manifest.oracle_command_overrides.lconvection, Some(0));
        assert_eq!(manifest.oracle_command_overrides.ctl, Some(5.0));
        assert_eq!(manifest.oracle_command_overrides.ifine, Some(4));
        assert_eq!(
            manifest.oracle_command_overrides.turbulence_formulation,
            OracleTurbulenceFormulation::AdaptiveWSigmaW
        );
        manifest
            .validate()
            .expect("WIND-UNI-002 overrides stay valid");
    }

    #[test]
    fn missing_oracle_override_is_rejected_without_default() {
        let mut manifest = make_minimal_manifest();
        manifest.oracle_command_overrides.lturbulence = None;
        let err = manifest
            .validate()
            .expect_err("missing lturbulence must fail");
        assert!(matches!(
            err,
            ValidationCaseError::MissingField {
                field: "oracle_command_overrides.lturbulence"
            }
        ));
    }

    #[test]
    fn oracle_flag_other_than_zero_or_one_is_rejected() {
        let mut manifest = make_minimal_manifest();
        manifest.oracle_command_overrides.lturbulence = Some(2);
        let err = manifest.validate().expect_err("flag 2 must fail");
        assert!(matches!(
            err,
            ValidationCaseError::InvalidPhysicsSwitches { .. }
        ));
    }

    #[test]
    fn oracle_switch_conflicting_with_physics_is_rejected() {
        let mut manifest = make_minimal_manifest();
        assert!(manifest.physics_switches.turbulence);
        manifest.oracle_command_overrides.lturbulence = Some(0);
        let err = manifest.validate().expect_err("conflict must fail");
        assert!(matches!(
            err,
            ValidationCaseError::InvalidPhysicsSwitches { .. }
        ));
    }

    #[test]
    fn oracle_command_rejects_nonexistent_deposition_decay_pseudo_flags() {
        let schema = load_validation_case_schema();
        for field in ["ldrydep", "lwetdep", "ldecay"] {
            let mut raw = minimal_manifest_json();
            raw["oracle_command_overrides"][field] = serde_json::json!(1);
            assert!(
                validate_json_schema_subset(&schema, &schema, &raw, "$").is_err(),
                "JSON Schema must reject nonexistent COMMAND pseudo-field {field}"
            );
            let err = parse_json_value(&raw)
                .expect_err("Rust contract must reject nonexistent COMMAND pseudo-field");
            assert!(
                err.to_string().contains(field) || err.to_string().contains("unknown field"),
                "unexpected error for {field}: {err}"
            );
        }
    }

    #[test]
    fn oracle_ctl_five_point_zero_is_accepted() {
        let manifest = make_minimal_manifest();
        assert_eq!(manifest.oracle_command_overrides.ctl, Some(5.0));
        assert_eq!(
            manifest.oracle_command_overrides.turbulence_formulation,
            OracleTurbulenceFormulation::AdaptiveWSigmaW
        );
        manifest
            .validate()
            .expect("CTL=5.0 corpus config stays valid");
    }

    #[test]
    fn zero_ctl_is_rejected_for_nonzero_timestep_division() {
        let mut manifest = make_minimal_manifest();
        manifest.oracle_command_overrides.ctl = Some(0.0);
        let err = manifest.validate().expect_err("ctl=0 must fail");
        assert!(matches!(
            err,
            ValidationCaseError::InvalidPhysicsSwitches { .. }
        ));
        assert!(err.to_string().contains("non-zero"));
        assert!(err.to_string().contains("readoptions_mod.f90:653"));
    }

    #[test]
    fn fixed_sync_ctl_requires_explicit_fixed_formulation() {
        let mut manifest = make_minimal_manifest();
        manifest.oracle_command_overrides.ctl = Some(-5.0);
        let err = manifest
            .validate()
            .expect_err("negative CTL with adaptive formulation must fail");
        assert!(err.to_string().contains("adaptive_w_sigw"));

        manifest.oracle_command_overrides.turbulence_formulation =
            OracleTurbulenceFormulation::FixedSyncW;
        manifest
            .validate()
            .expect("explicit fixed_sync_w mode validates");
    }

    #[test]
    fn small_positive_ctl_is_rejected_for_turbulence_formulation() {
        let mut manifest = make_minimal_manifest();
        assert_eq!(manifest.oracle_command_overrides.lturbulence, Some(1));
        manifest.oracle_command_overrides.ctl = Some(0.05);
        let err = manifest.validate().expect_err("ctl below w-sigw threshold");
        assert!(matches!(
            err,
            ValidationCaseError::InvalidPhysicsSwitches { .. }
        ));
        let rendered = err.to_string();
        assert!(
            rendered.contains("readoptions_mod.f90:645-650"),
            "error must cite the silent reformulation: {rendered}"
        );
    }

    #[test]
    fn small_positive_ctl_is_rejected_even_with_turbulence_disabled() {
        let mut manifest = make_minimal_manifest();
        manifest.physics_switches.turbulence = false;
        manifest.oracle_command_overrides.lturbulence = Some(0);
        manifest.oracle_command_overrides.ctl = Some(0.05);
        // The declared turbulence_formulation pins the w/sigw formulation, so a
        // sub-threshold CTL is inconsistent regardless of the LTURBULENCE flag.
        let err = manifest
            .validate()
            .expect_err("formulation inconsistency must fail");
        assert!(matches!(
            err,
            ValidationCaseError::InvalidPhysicsSwitches { .. }
        ));
    }

    #[test]
    fn ifine_zero_is_rejected_for_silent_clamp() {
        let mut manifest = make_minimal_manifest();
        manifest.oracle_command_overrides.ifine = Some(0);
        let err = manifest.validate().expect_err("ifine=0 must fail");
        assert!(matches!(
            err,
            ValidationCaseError::InvalidPhysicsSwitches { .. }
        ));
        assert!(err.to_string().contains("readoptions_mod.f90:624"));
    }

    #[test]
    fn unknown_turbulence_formulation_variant_is_rejected() {
        let mut raw: serde_json::Value =
            serde_json::to_value(make_minimal_manifest()).expect("serialize minimal");
        raw["oracle_command_overrides"]
            .as_object_mut()
            .expect("overrides object")
            .insert(
                "turbulence_formulation".to_string(),
                serde_json::json!("fixed_sync_unknown"),
            );
        let text = serde_json::to_string(&raw).expect("re-serialize");
        let err =
            ValidationCaseManifest::parse(&text, Path::new("badform.json")).expect_err("fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("adaptive_w_sigw") && rendered.contains("fixed_sync_w"),
            "error must enumerate the supported variants: {rendered}"
        );
    }

    #[test]
    fn missing_turbulence_formulation_is_rejected() {
        let mut raw: serde_json::Value =
            serde_json::to_value(make_minimal_manifest()).expect("serialize minimal");
        raw["oracle_command_overrides"]
            .as_object_mut()
            .expect("overrides object")
            .remove("turbulence_formulation");
        let text = serde_json::to_string(&raw).expect("re-serialize");
        let err =
            ValidationCaseManifest::parse(&text, Path::new("noform.json")).expect_err("fails");
        assert!(
            err.to_string().contains("turbulence_formulation"),
            "error must name the missing field: {err}"
        );
    }

    #[test]
    fn both_spellings_of_oracle_override_rejected_as_ambiguous() {
        let mut raw: serde_json::Value =
            serde_json::to_value(make_minimal_manifest()).expect("serialize minimal");
        raw["oracle_command_overrides"]
            .as_object_mut()
            .expect("overrides object")
            .insert("LTURBULENCE".to_string(), serde_json::json!(1));
        let text = serde_json::to_string(&raw).expect("re-serialize");
        let err = ValidationCaseManifest::parse(&text, Path::new("both.json")).expect_err("fails");
        assert!(matches!(
            err,
            ValidationCaseError::AmbiguousField {
                field: "oracle_command_overrides",
                ..
            }
        ));
        let rendered = err.to_string();
        assert!(
            rendered.contains("LTURBULENCE") && rendered.contains("lturbulence"),
            "error must name both spellings: {rendered}"
        );
    }

    #[test]
    fn legacy_only_uppercase_oracle_override_is_rejected() {
        let mut raw: serde_json::Value =
            serde_json::to_value(make_minimal_manifest()).expect("serialize minimal");
        raw["oracle_command_overrides"]
            .as_object_mut()
            .expect("overrides object")
            .remove("lturbulence");
        raw["oracle_command_overrides"]
            .as_object_mut()
            .expect("overrides object")
            .insert("LTURBULENCE".to_string(), serde_json::json!(1));
        let text = serde_json::to_string(&raw).expect("re-serialize");
        let err =
            ValidationCaseManifest::parse(&text, Path::new("legacy.json")).expect_err("fails");
        let rendered = err.to_string();
        assert!(
            rendered.contains("LTURBULENCE"),
            "legacy spelling must fail naming the key: {rendered}"
        );
    }

    #[test]
    fn oracle_override_schema_and_rust_required_fields_are_in_parity() {
        let schema = load_validation_case_schema();
        for field in [
            "turbulence_formulation",
            "lturbulence",
            "lconvection",
            "ctl",
            "ifine",
            "lsynctime_s",
        ] {
            let mut raw = minimal_manifest_json();
            raw["oracle_command_overrides"]
                .as_object_mut()
                .expect("oracle overrides object")
                .remove(field);

            let schema_error = validate_json_schema_subset(&schema, &schema, &raw, "$")
                .expect_err("JSON Schema must reject missing oracle field");
            assert!(
                schema_error.contains(field),
                "schema error must name missing {field}: {schema_error}"
            );

            let rust_error = parse_json_value(&raw)
                .expect_err("Rust contract must reject the same missing oracle field");
            assert!(
                rust_error.to_string().contains(field),
                "Rust error must name missing {field}: {rust_error}"
            );
        }

        let mut missing_block = minimal_manifest_json();
        missing_block
            .as_object_mut()
            .expect("manifest object")
            .remove("oracle_command_overrides");
        assert!(
            validate_json_schema_subset(&schema, &schema, &missing_block, "$").is_err(),
            "JSON Schema must reject a missing oracle override block"
        );
        let err = parse_json_value(&missing_block)
            .expect_err("Rust deserialization must reject a missing oracle override block");
        assert!(
            err.to_string().contains("oracle_command_overrides"),
            "Rust error must name missing oracle block: {err}"
        );
    }

    #[test]
    fn oracle_override_schema_and_rust_numeric_semantics_are_in_parity() {
        let schema = load_validation_case_schema();

        let mut too_large_ifine = minimal_manifest_json();
        too_large_ifine["oracle_command_overrides"]["ifine"] = serde_json::json!(11);
        assert!(
            validate_json_schema_subset(&schema, &schema, &too_large_ifine, "$").is_err(),
            "JSON Schema must enforce IFINE <= 10"
        );
        assert!(
            parse_json_value(&too_large_ifine).is_err(),
            "Rust contract must enforce IFINE <= 10"
        );

        let mut adaptive_negative = minimal_manifest_json();
        adaptive_negative["oracle_command_overrides"]["ctl"] = serde_json::json!(-5.0);
        assert!(
            validate_json_schema_subset(&schema, &schema, &adaptive_negative, "$").is_err(),
            "adaptive_w_sigw with negative CTL must fail JSON Schema"
        );
        assert!(
            parse_json_value(&adaptive_negative).is_err(),
            "adaptive_w_sigw with negative CTL must fail Rust"
        );

        let mut fixed_positive = minimal_manifest_json();
        fixed_positive["oracle_command_overrides"]["turbulence_formulation"] =
            serde_json::json!("fixed_sync_w");
        fixed_positive["oracle_command_overrides"]["ctl"] = serde_json::json!(5.0);
        assert!(
            validate_json_schema_subset(&schema, &schema, &fixed_positive, "$").is_err(),
            "fixed_sync_w with positive CTL must fail JSON Schema"
        );
        assert!(
            parse_json_value(&fixed_positive).is_err(),
            "fixed_sync_w with positive CTL must fail Rust"
        );

        let mut fixed_valid = minimal_manifest_json();
        fixed_valid["oracle_command_overrides"]["turbulence_formulation"] =
            serde_json::json!("fixed_sync_w");
        fixed_valid["oracle_command_overrides"]["ctl"] = serde_json::json!(-5.0);
        validate_json_schema_subset(&schema, &schema, &fixed_valid, "$")
            .expect("JSON Schema accepts explicit fixed_sync_w semantics");
        parse_json_value(&fixed_valid)
            .expect("Rust accepts the same explicit fixed_sync_w semantics");
    }
}
