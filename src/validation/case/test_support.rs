//! Shared test fixtures and existing schema-subset evaluator.

use super::*;
use std::path::Path;

fn json_schema_type_matches(expected: &str, value: &serde_json::Value) -> bool {
    match expected {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "integer" => value.as_i64().is_some() || value.as_u64().is_some(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        _ => false,
    }
}

/// Minimal Draft 2020-12 evaluator for the keyword subset used by the
/// checked-in validation-case schema. This keeps the schema test
/// dependency-free while validating the actual schema document, not a
/// second hand-written fixture shape.
pub(super) fn validate_json_schema_subset(
    root: &serde_json::Value,
    schema: &serde_json::Value,
    value: &serde_json::Value,
    path: &str,
) -> Result<(), String> {
    let node = schema
        .as_object()
        .ok_or_else(|| format!("{path}: schema node is not an object"))?;

    if let Some(reference) = node.get("$ref").and_then(serde_json::Value::as_str) {
        let pointer = reference
            .strip_prefix('#')
            .ok_or_else(|| format!("{path}: only local schema refs are supported: {reference}"))?;
        let target = root
            .pointer(pointer)
            .ok_or_else(|| format!("{path}: unresolved schema ref {reference}"))?;
        return validate_json_schema_subset(root, target, value, path);
    }

    // Report requirements owned by this object before evaluating dependent
    // branches so a missing discriminator is not hidden by a generic
    // `oneOf` mismatch.
    if let Some(object) = value.as_object() {
        if let Some(required) = node.get("required").and_then(serde_json::Value::as_array) {
            for key in required.iter().filter_map(serde_json::Value::as_str) {
                if !object.contains_key(key) {
                    return Err(format!("{path}: missing required property {key}"));
                }
            }
        }
    }

    if let Some(branches) = node.get("oneOf").and_then(serde_json::Value::as_array) {
        let matches = branches
            .iter()
            .filter(|branch| validate_json_schema_subset(root, branch, value, path).is_ok())
            .count();
        if matches != 1 {
            return Err(format!(
                "{path}: oneOf expected exactly one matching branch, got {matches}"
            ));
        }
    }

    if let Some(expected) = node.get("const") {
        if expected != value {
            return Err(format!("{path}: expected const {expected}, got {value}"));
        }
    }

    if let Some(allowed) = node.get("enum").and_then(serde_json::Value::as_array) {
        if !allowed.iter().any(|candidate| candidate == value) {
            return Err(format!("{path}: value {value} is not in enum {allowed:?}"));
        }
    }

    if let Some(expected_type) = node.get("type").and_then(serde_json::Value::as_str) {
        if !json_schema_type_matches(expected_type, value) {
            return Err(format!(
                "{path}: expected JSON type {expected_type}, got {value}"
            ));
        }
    }

    if let Some(object) = value.as_object() {
        let properties = node
            .get("properties")
            .and_then(serde_json::Value::as_object);
        for (key, child) in object {
            if let Some(child_schema) = properties.and_then(|props| props.get(key)) {
                validate_json_schema_subset(root, child_schema, child, &format!("{path}.{key}"))?;
            } else if node
                .get("additionalProperties")
                .and_then(serde_json::Value::as_bool)
                == Some(false)
            {
                return Err(format!("{path}: unknown property {key}"));
            }
        }
    }

    if let Some(array) = value.as_array() {
        if let Some(min_items) = node.get("minItems").and_then(serde_json::Value::as_u64) {
            if array.len() < min_items as usize {
                return Err(format!("{path}: fewer than {min_items} items"));
            }
        }
        if let Some(max_items) = node.get("maxItems").and_then(serde_json::Value::as_u64) {
            if array.len() > max_items as usize {
                return Err(format!("{path}: more than {max_items} items"));
            }
        }
        if let Some(item_schema) = node.get("items") {
            for (index, child) in array.iter().enumerate() {
                validate_json_schema_subset(root, item_schema, child, &format!("{path}[{index}]"))?;
            }
        }
    }

    if let Some(text) = value.as_str() {
        if let Some(min_length) = node.get("minLength").and_then(serde_json::Value::as_u64) {
            if text.chars().count() < min_length as usize {
                return Err(format!("{path}: string shorter than {min_length}"));
            }
        }
        if let Some(max_length) = node.get("maxLength").and_then(serde_json::Value::as_u64) {
            if text.chars().count() > max_length as usize {
                return Err(format!("{path}: string longer than {max_length}"));
            }
        }
        if let Some(pattern) = node.get("pattern").and_then(serde_json::Value::as_str) {
            match pattern {
                "^[0-9]{14}$" => {
                    if text.len() != 14 || !text.bytes().all(|b| b.is_ascii_digit()) {
                        return Err(format!("{path}: string does not match {pattern}"));
                    }
                }
                other => {
                    return Err(format!(
                        "{path}: schema test evaluator does not support pattern {other}"
                    ));
                }
            }
        }
    }

    if let Some(number) = value.as_f64() {
        if let Some(minimum) = node.get("minimum").and_then(serde_json::Value::as_f64) {
            if number < minimum {
                return Err(format!("{path}: {number} is below minimum {minimum}"));
            }
        }
        if let Some(minimum) = node
            .get("exclusiveMinimum")
            .and_then(serde_json::Value::as_f64)
        {
            if number <= minimum {
                return Err(format!(
                    "{path}: {number} is not greater than exclusiveMinimum {minimum}"
                ));
            }
        }
        if let Some(maximum) = node.get("maximum").and_then(serde_json::Value::as_f64) {
            if number > maximum {
                return Err(format!("{path}: {number} exceeds maximum {maximum}"));
            }
        }
        if let Some(maximum) = node
            .get("exclusiveMaximum")
            .and_then(serde_json::Value::as_f64)
        {
            if number >= maximum {
                return Err(format!(
                    "{path}: {number} is not less than exclusiveMaximum {maximum}"
                ));
            }
        }
        if let Some(multiple) = node.get("multipleOf").and_then(serde_json::Value::as_f64) {
            if multiple <= 0.0 {
                return Err(format!("{path}: schema multipleOf must be > 0"));
            }
            let quotient = number / multiple;
            if (quotient - quotient.round()).abs() > 1e-9 {
                return Err(format!("{path}: {number} is not a multiple of {multiple}"));
            }
        }
    }

    Ok(())
}

pub(super) fn load_validation_case_schema() -> serde_json::Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(VALIDATION_CASE_SCHEMA_PATH);
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {}: {e}", path.display()))
}

pub(super) fn make_minimal_manifest() -> ValidationCaseManifest {
    ValidationCaseManifest {
        schema_version: VALIDATION_CASE_SCHEMA_VERSION,
        case_id: "TEST-001".to_string(),
        description: "Minimal test case".to_string(),
        domain: DomainSpec {
            nx: 32,
            ny: 32,
            nz: 8,
            dx_deg: 0.1,
            dy_deg: 0.1,
            xlon0_deg: 9.5,
            ylat0_deg: 8.5,
            horizontal_ref: HorizontalCoordRef::GeographicLonLatDegrees,
            wind_heights_m: vec![50.0, 100.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0, 20000.0],
            wind_heights_ref: VerticalRef::Agl,
        },
        release: ReleaseSpec {
            geometry: SourceGeometry::Point {
                lon_deg: 10.0,
                lat_deg: 10.0,
                z_m: 50.0,
            },
            vertical_ref: VerticalRef::Agl,
            timing: ReleaseTiming::Instant {
                at: "20240101000000".to_string(),
            },
            species: SpeciesRef {
                id: "SPECIES_024".to_string(),
                physics_contract: SpeciesPhysicsContractRef {
                    profile: SpeciesPhysicsProfile::Species024InertV1,
                    id: SPECIES_024_INERT_CONTRACT_ID.to_string(),
                    version: 1,
                    path: SPECIES_024_INERT_CONTRACT_PATH.to_string(),
                    git_blob_sha: SPECIES_024_INERT_CONTRACT_BLOB.to_string(),
                },
            },
            inventory: ReleaseInventory {
                quantity_kg: 1.0,
                unit: MassUnit::Kg,
            },
            particle_count: 1000,
            mass_kg_per_particle: None,
        },
        wind: WindSpec::Uniform {
            u_m_s: 5.0,
            v_m_s: -3.0,
            w_m_s: 0.0,
        },
        surface: Some(SurfaceSpec {
            u10_m_s: 5.0,
            v10_m_s: -3.0,
            surface_pressure_pa: 101325.0,
            temperature_2m_k: 289.0,
            dewpoint_2m_k: 284.0,
            sensible_heat_flux_w_m2: 0.0,
            solar_radiation_w_m2: 120.0,
            surface_stress_n_m2: 0.2,
            friction_velocity_m_s: 0.35,
            convective_velocity_scale_m_s: 0.0,
            mixing_height_m: 1500.0,
            tropopause_height_m: 10000.0,
            inv_obukhov_length_per_m: 0.0,
            precip_large_scale_mm_h: 0.0,
            precip_convective_mm_h: 0.0,
        }),
        integration: IntegrationSpec {
            start: "20240101000000".to_string(),
            dt_s: 300.0,
            steps: 12,
            total_s: 3600.0,
        },
        physics_switches: PhysicsSwitches {
            turbulence: true,
            convection: false,
            dry_deposition: false,
            wet_deposition: false,
            decay: false,
        },
        simulation_direction: SimulationDirection::Forward,
        output: OutputSpec {
            interval_s: 1800,
            averaging_window_s: 1800,
            sampling_interval_s: 300,
            quantity: OutputQuantity::TimeAveragedMassConcentrationKgM3,
        },
        output_grid: Some(OutputGridSpec {
            nx: 32,
            ny: 32,
            nz: 1,
            dx_deg: 0.1,
            dy_deg: 0.1,
            xlon0_deg: 9.5,
            ylat0_deg: 8.5,
            horizontal_ref: HorizontalCoordRef::GeographicLonLatDegrees,
            heights_m: vec![100.0],
            heights_ref: VerticalRef::Agl,
        }),
        deposition: None,
        units: UnitsSpec {
            wind: "m/s".to_string(),
            displacement: None,
            pressure: Some("Pa".to_string()),
            temperature: Some("K".to_string()),
            heat_flux: Some("W/m2".to_string()),
            height: Some("m".to_string()),
            mass: Some("kg".to_string()),
            time: Some("s".to_string()),
            shear: Some("1/s".to_string()),
            inv_obukhov: Some("1/m".to_string()),
            deposition_velocity: Some("m/s".to_string()),
            scavenging_coefficient: Some("1/s".to_string()),
            concentration: Some("kg/m3".to_string()),
        },
        stochastic: StochasticIdentitySpec {
            candidate_philox: Some(CandidatePhiloxIdentity {
                base_key: [3737180555, 305419896],
                base_counter: [0, 0, 0, 0],
                count: 10,
                derivation: CandidatePhiloxDerivation::WrappingAddKey0V1,
            }),
            oracle_seed: Some(OracleSeedIdentity {
                kind: OracleKind::SeedableValidationOracle,
                strategy: Some(OracleStrategyRef::canonical()),
                mode: OracleSeedMode::RequestedIdentity,
                seed: Some(1),
                repetitions: 5,
            }),
        },
        validation_definition_refs: ValidationDefinitionRefs {
            metric_contracts: vec![ValidationDefinitionRef {
                id: "evaluation-metrics".to_string(),
                version: "report-schema-1.0.0".to_string(),
                path: "scripts/evaluate/metrics.py".to_string(),
            }],
            threshold_contracts: vec![
                ValidationDefinitionRef {
                    id: "scientific-thresholds".to_string(),
                    version: "v1".to_string(),
                    path: "evaluation/thresholds/scientific-thresholds-v1.json".to_string(),
                },
                ValidationDefinitionRef {
                    id: "corpus-thresholds".to_string(),
                    version: "1".to_string(),
                    path: "fixtures/corpus/thresholds.json".to_string(),
                },
            ],
        },
        execution_profile: ExecutionProfileRef {
            id: "flexpart-11.1-single-thread".to_string(),
            version: 1,
            manifest_path: "reference/flexpart-11.1.json".to_string(),
        },
        oracle_execution: OracleExecutionPolicy::Required,
        oracle_meteorology_profile: OracleMeteorologyProfileRef {
            id: SYNTHETIC_ORACLE_METEOROLOGY_PROFILE_ID.to_string(),
            version: SYNTHETIC_ORACLE_METEOROLOGY_PROFILE_VERSION,
            manifest_path: SYNTHETIC_ORACLE_METEOROLOGY_PROFILE_PATH.to_string(),
        },
        candidate_physics_profile: CandidatePhysicsProfileRef::canonical(),
        oracle_command_overrides: OracleCommandOverrides {
            turbulence_formulation: OracleTurbulenceFormulation::AdaptiveWSigmaW,
            lturbulence: Some(1),
            ctl: Some(5.0),
            ifine: Some(4),
            lsynctime_s: Some(300),
            lconvection: Some(0),
        },
        expected_artifacts: ExpectedArtifacts {
            required: vec![
                ExpectedArtifactRequirement {
                    id: "candidate.raw".to_string(),
                    producer: ArtifactProducer::Candidate,
                    class: ArtifactClass::RawModelOutput,
                },
                ExpectedArtifactRequirement {
                    id: "candidate.decoded".to_string(),
                    producer: ArtifactProducer::Candidate,
                    class: ArtifactClass::DecodedModelOutput,
                },
                ExpectedArtifactRequirement {
                    id: "oracle.raw".to_string(),
                    producer: ArtifactProducer::Oracle,
                    class: ArtifactClass::RawModelOutput,
                },
                ExpectedArtifactRequirement {
                    id: "oracle.decoded".to_string(),
                    producer: ArtifactProducer::Oracle,
                    class: ArtifactClass::DecodedModelOutput,
                },
                ExpectedArtifactRequirement {
                    id: "comparison.report".to_string(),
                    producer: ArtifactProducer::ValidationPipeline,
                    class: ArtifactClass::ComparisonReport,
                },
                ExpectedArtifactRequirement {
                    id: "run.manifest".to_string(),
                    producer: ArtifactProducer::ValidationPipeline,
                    class: ArtifactClass::RunManifest,
                },
            ],
        },
        representation_differences: RepresentationDifferences::default(),
        require_source_containment: true,
        notes: vec![],
    }
}

pub(super) fn make_valid_dry_deposition_manifest() -> ValidationCaseManifest {
    let mut manifest = make_minimal_manifest();
    manifest.physics_switches.dry_deposition = true;
    manifest.release.species.id = "SPECIES_040".to_string();
    manifest.release.species.physics_contract = SpeciesPhysicsContractRef {
        profile: SpeciesPhysicsProfile::Species040DryConstantV1,
        id: SPECIES_040_DRY_CONTRACT_ID.to_string(),
        version: 1,
        path: SPECIES_040_DRY_CONTRACT_PATH.to_string(),
        git_blob_sha: SPECIES_040_DRY_CONTRACT_BLOB.to_string(),
    };
    manifest.deposition = Some(DepositionSpec {
        dry_deposition_velocity_m_s: 0.02,
        dry_reference_height_m: Some(15.0),
        wet_scavenging_coefficient_s_inv: 0.0,
        wet_precipitating_fraction: 0.0,
    });
    manifest
        .validate()
        .expect("canonical dry-deposition fixture must validate");
    manifest
}

pub(super) fn load_checked_in_case(case_id: &str) -> ValidationCaseManifest {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("corpus")
        .join("cases")
        .join(format!("{case_id}.json"));
    ValidationCaseManifest::load_from_file(&path).unwrap_or_else(|e| panic!("load {case_id}: {e}"))
}

pub(super) fn minimal_manifest_json() -> serde_json::Value {
    serde_json::to_value(make_minimal_manifest()).expect("serialize minimal")
}

pub(super) fn valid_real_weather_manifest_json() -> serde_json::Value {
    serde_json::to_value(load_checked_in_case("ETEX-MINI-013"))
        .expect("serialize valid real-weather fixture")
}

pub(super) fn parse_json_value(
    value: &serde_json::Value,
) -> Result<ValidationCaseManifest, ValidationCaseError> {
    let text = serde_json::to_string(value).expect("re-serialize");
    ValidationCaseManifest::parse(&text, Path::new("test.json"))
}

pub(super) const ALL_CHECKED_IN_CASES: &[&str] = &[
    "ADV-ANA-001",
    "WIND-UNI-002",
    "WIND-SHEAR-003",
    "PBL-STABLE-004",
    "PBL-NEUTRAL-005",
    "PBL-UNSTABLE-006",
    "DRY-007",
    "WET-008",
    "REPEAT-009",
    "ETEX-MINI-013",
];
