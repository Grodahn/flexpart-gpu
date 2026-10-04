//! Issue #35: GPU gravitational settling velocity parity against pinned FLEXPART 11.1.
//!
//! The WGSL kernel `src/shaders/settling_velocity.wgsl` executes the spherical
//! `settling_mod::get_settling` formulation on the device. Host validation
//! uses per-species #10 carrier properties (`PDENSITY`/`PDIA`, `PSHAPE == 0`
//! only) so grid, unit and domain semantics are identical. Evidence uses the
//! #91 `GpuCalculationEvidence` model. No Rust CPU settling implementation
//! participates in acceptance; the pinned Fortran oracle (see
//! `oracle/settling_oracle.f90` and `fixtures/settling/oracle-v1.json`) is
//! authoritative. Production composition uses `encode_settling_velocities`
//! (caller-owned encoder, no submit/wait/readback).

use flexpart_gpu::config::SpeciesConfig;
use flexpart_gpu::gpu::{
    build_settling_gpu_row, compute_settling_velocities_gpu, default_settling_comparison_policy,
    settling_inputs_sha256, settling_shader_sha256, ComparisonPolicy, GpuCalculationPath,
    GpuEvidenceError, GpuExecutionStatus, PinnedOracleEvidence, SettlingCarrier, SettlingGpuReport,
    SettlingQuery, SettlingReportSchema, SettlingVelocityKernel,
    SETTLING_GPU_CANDIDATE_DESCRIPTION, SETTLING_GPU_IMPLEMENTATION_ID,
    SETTLING_GPU_REPORT_SCHEMA_ID, SETTLING_GPU_REPORT_SCHEMA_VERSION,
    SETTLING_ORACLE_IMPLEMENTATION_ID, SETTLING_ORACLE_REVISION,
};
use flexpart_gpu::particles::{ParticleInit, ParticleStore, MAX_SPECIES};
use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

#[derive(Debug, Deserialize)]
struct CanonicalDoc {
    schema: SchemaId,
    pinned_flexpart_revision: String,
    vectors: Vec<CanonicalVector>,
}

#[derive(Debug, Deserialize)]
struct SchemaId {
    id: String,
    version: u32,
}

#[derive(Debug, Deserialize)]
struct CanonicalVector {
    id: String,
    species: String,
    diameter_um: f64,
    density_kg_m3: f64,
    temperature_k: f64,
    air_density_kg_m3: f64,
}

#[derive(Debug, Deserialize)]
struct OracleDoc {
    schema: SchemaId,
    pinned_implementation: String,
    pinned_revision: String,
    harness: String,
    harness_sha256: String,
    executable_sha256: String,
    canonical_sha256: String,
    output_sha256: String,
    values: Vec<OracleValue>,
}

#[derive(Debug, Deserialize)]
struct OracleValue {
    id: String,
    settling_velocity_m_s: f64,
}

fn fixture_dir() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join("settling")
}

fn read_canonical() -> CanonicalDoc {
    let text = std::fs::read_to_string(fixture_dir().join("canonical-vectors-v1.json"))
        .expect("canonical settling fixture must exist");
    serde_json::from_str(&text).expect("canonical fixture must parse")
}

fn read_oracle() -> OracleDoc {
    let text = std::fs::read_to_string(fixture_dir().join("oracle-v1.json"))
        .expect("oracle settling fixture must exist");
    let oracle: OracleDoc = serde_json::from_str(&text).expect("oracle fixture must parse");
    audit_fixtures(&read_canonical(), &oracle);
    oracle
}

fn hash_bytes(bytes: &[u8]) -> String {
    use sha2::Digest as _;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

fn audit_fixtures(canonical: &CanonicalDoc, oracle: &OracleDoc) {
    assert_eq!(
        canonical.schema.id,
        "flexpart-gpu.settling-canonical-vectors"
    );
    assert_eq!(canonical.schema.version, 1);
    assert_eq!(oracle.schema.id, "flexpart-gpu.settling-oracle");
    assert_eq!(oracle.schema.version, 1);
    assert_eq!(canonical.pinned_flexpart_revision, SETTLING_ORACLE_REVISION);
    assert_eq!(oracle.pinned_revision, SETTLING_ORACLE_REVISION);
    assert_eq!(
        oracle.pinned_implementation,
        SETTLING_ORACLE_IMPLEMENTATION_ID
    );
    assert_eq!(oracle.harness, "oracle/settling_oracle.f90");
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(root.join("reference/flexpart-11.1.json")).expect("manifest"),
    )
    .expect("manifest JSON");
    assert_eq!(manifest["pinned_commit"], SETTLING_ORACLE_REVISION);
    assert_eq!(
        oracle.canonical_sha256,
        hash_bytes(
            &std::fs::read(fixture_dir().join("canonical-vectors-v1.json"))
                .expect("canonical bytes"),
        )
    );
    let harness = std::fs::read_to_string(root.join(&oracle.harness)).expect("oracle driver");
    assert_eq!(
        oracle.harness_sha256,
        hash_bytes(harness.replace("\r\n", "\n").as_bytes())
    );
    let raw = std::fs::read(fixture_dir().join("oracle-output-v1.txt")).expect("raw oracle output");
    assert_eq!(oracle.output_sha256, hash_bytes(&raw));
    let raw_values: BTreeMap<_, _> = std::str::from_utf8(&raw)
        .expect("raw UTF-8")
        .lines()
        .map(|line| {
            let fields: Vec<_> = line.split_whitespace().collect();
            assert_eq!(fields.len(), 2);
            (
                fields[0].to_string(),
                fields[1].parse::<f64>().expect("raw velocity"),
            )
        })
        .collect();
    let canonical_ids: std::collections::BTreeSet<_> =
        canonical.vectors.iter().map(|v| &v.id).collect();
    let oracle_ids: std::collections::BTreeSet<_> = oracle.values.iter().map(|v| &v.id).collect();
    assert_eq!(
        canonical_ids.len(),
        canonical.vectors.len(),
        "unique canonical ids"
    );
    assert_eq!(oracle_ids.len(), oracle.values.len(), "unique oracle ids");
    assert_eq!(canonical_ids, oracle_ids, "exact oracle coverage");
    assert_eq!(raw_values.len(), oracle.values.len());
    for value in &oracle.values {
        // serde_json and Rust's decimal parser can differ by one f64 ULP;
        // the authoritative Fortran output and GPU comparison both use f32.
        #[allow(clippy::cast_possible_truncation)]
        let raw_value = raw_values[&value.id] as f32;
        #[allow(clippy::cast_possible_truncation)]
        let decoded_value = value.settling_velocity_m_s as f32;
        assert_eq!(raw_value.to_bits(), decoded_value.to_bits());
        assert!(value.settling_velocity_m_s.is_finite() && value.settling_velocity_m_s < 0.0);
    }
}

fn candidate_revision() -> String {
    let output = Command::new("git")
        .args([
            "-c",
            &format!("safe.directory={}", env!("CARGO_MANIFEST_DIR")),
            "rev-parse",
            "HEAD",
        ])
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("read candidate Git revision");
    assert!(output.status.success(), "git rev-parse HEAD must succeed");
    let actual = String::from_utf8(output.stdout)
        .expect("Git revision must be UTF-8")
        .trim()
        .to_string();
    if let Ok(revision) = std::env::var("FLEXPART_GPU_CANDIDATE_REVISION") {
        assert_eq!(
            revision.trim(),
            actual,
            "candidate revision must identify this checkout"
        );
    }
    actual
}

fn try_gpu_context() -> Option<flexpart_gpu::gpu::GpuContext> {
    match pollster::block_on(flexpart_gpu::gpu::GpuContext::new()) {
        Ok(ctx) => Some(ctx),
        Err(flexpart_gpu::gpu::GpuError::NoAdapter) => {
            assert_ne!(
                std::env::var("FLEXPART_GPU_REQUIRE_SETTLING").as_deref(),
                Ok("1"),
                "required settling WGSL adapter missing"
            );
            assert_ne!(
                std::env::var("FLEXPART_GPU_SOFTWARE").as_deref(),
                Ok("1"),
                "required settling WGSL adapter missing"
            );
            None
        }
        Err(err) => panic!("unexpected GPU init error: {err}"),
    }
}

fn comparison_policy() -> ComparisonPolicy {
    default_settling_comparison_policy().expect("settling comparison policy must be valid")
}

fn oracle_evidence(oracle: &OracleDoc) -> PinnedOracleEvidence {
    PinnedOracleEvidence {
        implementation_id: SETTLING_ORACLE_IMPLEMENTATION_ID.to_string(),
        revision: SETTLING_ORACLE_REVISION.to_string(),
        executable_sha256: oracle.executable_sha256.clone(),
        output_sha256: oracle.output_sha256.clone(),
    }
}

fn aerosol_species(name: &str, density: f64, diameter_um: f64) -> SpeciesConfig {
    SpeciesConfig {
        name: name.to_string(),
        version: None,
        molecular_weight: None,
        dry_deposition_velocity: None,
        decay_constant: None,
        half_life_s: None,
        wet_a_gas: None,
        wet_b_gas: None,
        crain_aero: None,
        csnow_aero: None,
        ccn_aero: None,
        in_aero: None,
        relative_diffusivity: None,
        henry: None,
        surface_reactivity_f0: None,
        particle_density_kg_m3: Some(density),
        mean_diameter_um: Some(diameter_um),
        diameter_sigma: Some(2.0),
        source_file: None,
        raw: BTreeMap::new(),
    }
}

#[test]
fn test_settling_fixtures_are_pinned_and_consistent() {
    let canonical = read_canonical();
    let oracle = read_oracle();
    assert_eq!(
        canonical.pinned_flexpart_revision, SETTLING_ORACLE_REVISION,
        "canonical fixture must bind the pinned revision"
    );
    assert_eq!(
        oracle.pinned_revision, SETTLING_ORACLE_REVISION,
        "oracle fixture must bind the pinned revision"
    );
    assert_eq!(
        oracle.pinned_implementation, SETTLING_ORACLE_IMPLEMENTATION_ID,
        "oracle fixture must identify the pinned routine"
    );
    assert_eq!(
        canonical.vectors.len(),
        32,
        "canonical vector count is fixed"
    );
    assert_eq!(oracle.values.len(), 32, "oracle value count is fixed");
    for vector in &canonical.vectors {
        let value = oracle
            .values
            .iter()
            .find(|value| value.id == vector.id)
            .unwrap_or_else(|| panic!("oracle value missing for {}", vector.id));
        assert!(
            value.settling_velocity_m_s.is_finite() && value.settling_velocity_m_s < 0.0,
            "oracle settling must be finite and downward for {}",
            vector.id
        );
    }
}

#[test]
fn test_settling_carriers_use_per_species_properties() {
    // Two species with identical diameters but different densities must yield
    // different carriers; no shared slot/default may override per-species inputs.
    let light = aerosol_species("AERO-light", 800.0, 5.0);
    let dense = aerosol_species("AERO-dense", 2500.0, 5.0);
    let light_carrier = SettlingCarrier::from_species_config(&light).expect("light carrier valid");
    let dense_carrier = SettlingCarrier::from_species_config(&dense).expect("dense carrier valid");
    assert!(
        (f64::from(dense_carrier.particle_density_kg_m3)
            - f64::from(light_carrier.particle_density_kg_m3))
        .abs()
            > 100.0,
        "per-species densities must be preserved"
    );
    assert_eq!(
        light_carrier.diameter_um.to_bits(),
        dense_carrier.diameter_um.to_bits(),
        "diameters match by construction"
    );

    // Gas and passive tracers cannot settle.
    let gas = SpeciesConfig {
        relative_diffusivity: Some(0.8),
        ..aerosol_species("GAS", 1000.0, 5.0)
    };
    let gas = SpeciesConfig {
        particle_density_kg_m3: None,
        mean_diameter_um: None,
        ..gas
    };
    assert!(SettlingCarrier::from_species_config(&gas).is_err());

    // Non-spherical shapes fail closed and are never approximated.
    let mut raw = BTreeMap::new();
    raw.insert("pshape".to_string(), "2".to_string());
    let non_spherical = SpeciesConfig {
        raw,
        ..aerosol_species("FIBER", 1000.0, 10.0)
    };
    assert!(SettlingCarrier::from_species_config(&non_spherical).is_err());
    for shape in ["invalid", "NaN", "1D0"] {
        let mut malformed = aerosol_species("INVALID-SHAPE", 1000.0, 10.0);
        malformed
            .raw
            .insert("pshape".to_string(), shape.to_string());
        assert!(SettlingCarrier::from_species_config(&malformed).is_err());
    }
    let mut conflicting = aerosol_species("CONFLICTING-SHAPE", 1000.0, 10.0);
    conflicting
        .raw
        .insert("pshape".to_string(), "0".to_string());
    conflicting.raw.insert("shape".to_string(), "2".to_string());
    assert!(SettlingCarrier::from_species_config(&conflicting).is_err());
    let mut mixed = aerosol_species("MIXED-GAS", 1000.0, 10.0);
    mixed.relative_diffusivity = Some(0.8);
    assert!(SettlingCarrier::from_species_config(&mixed).is_err());
    assert!(
        SettlingCarrier::from_species_config(&aerosol_species("ROUNDING", 1000.0, 100.000001))
            .is_err()
    );
    for sigma in [None, Some(1.0), Some(f64::NAN)] {
        let mut invalid = aerosol_species("INVALID-WIDTH", 1000.0, 10.0);
        invalid.diameter_sigma = sigma;
        assert!(SettlingCarrier::from_species_config(&invalid).is_err());
    }

    // Out-of-domain carriers fail closed.
    let too_small = aerosol_species("TOO-SMALL", 1000.0, 0.001);
    assert!(SettlingCarrier::from_species_config(&too_small).is_err());
    let too_large = aerosol_species("TOO-LARGE", 1000.0, 500.0);
    assert!(SettlingCarrier::from_species_config(&too_large).is_err());
}

#[test]
fn test_settling_invalid_inputs_fail_closed() {
    let carrier = SettlingCarrier {
        particle_density_kg_m3: 1000.0,
        diameter_um: 10.0,
    };
    assert!(SettlingQuery::new(carrier, f32::NAN, 1.2).is_err());
    assert!(SettlingQuery::new(carrier, 293.15, f32::INFINITY).is_err());
    assert!(SettlingQuery::new(carrier, 0.0, 1.2).is_err());
    assert!(SettlingQuery::new(carrier, 293.15, 0.0).is_err());
    let bad_carrier = SettlingCarrier {
        particle_density_kg_m3: 0.0,
        diameter_um: 10.0,
    };
    assert!(SettlingQuery::new(bad_carrier, 293.15, 1.2).is_err());
}

#[test]
fn test_settling_gpu_matches_pinned_oracle() {
    let ctx = match try_gpu_context() {
        Some(ctx) => ctx,
        None => return,
    };
    let canonical = read_canonical();
    let oracle = read_oracle();
    let policy = comparison_policy();
    let revision = candidate_revision();
    let kernel = SettlingVelocityKernel::new(&ctx).expect("settling kernel compiles");

    // One dispatch for all vectors proves per-species independence: each row
    // carries its own #10 diameter/density through the same WGSL execution.
    let mut queries = Vec::with_capacity(canonical.vectors.len());
    for vector in &canonical.vectors {
        #[allow(clippy::cast_possible_truncation)]
        let carrier = SettlingCarrier::from_species_config(&aerosol_species(
            &vector.species,
            vector.density_kg_m3,
            vector.diameter_um,
        ))
        .expect("canonical species carrier");
        #[allow(clippy::cast_possible_truncation)]
        let query = SettlingQuery::new(
            carrier,
            vector.temperature_k as f32,
            vector.air_density_kg_m3 as f32,
        )
        .unwrap_or_else(|err| panic!("canonical vector {} must validate: {err}", vector.id));
        queries.push(query);
    }
    let gpu_values = pollster::block_on(compute_settling_velocities_gpu(&ctx, &queries, &kernel))
        .expect("GPU settling dispatch succeeds");
    assert_eq!(gpu_values.len(), canonical.vectors.len());

    let oracle_evidence = oracle_evidence(&oracle);
    let mut rows = Vec::with_capacity(queries.len());
    for ((vector, query), gpu_value) in canonical
        .vectors
        .iter()
        .zip(queries.iter())
        .zip(gpu_values.iter())
    {
        let oracle_value = oracle
            .values
            .iter()
            .find(|value| value.id == vector.id)
            .unwrap_or_else(|| panic!("oracle value missing for {}", vector.id))
            .settling_velocity_m_s;
        #[allow(clippy::cast_possible_truncation)]
        let oracle_f32 = oracle_value as f32;
        // Sign convention: settling is strictly downward (negative).
        assert!(
            *gpu_value < 0.0,
            "GPU settling must be downward for {}",
            vector.id
        );
        let row = build_settling_gpu_row(
            &ctx,
            &vector.id,
            &vector.species,
            *query,
            *gpu_value,
            oracle_f32,
            policy,
            &revision,
            oracle_evidence.clone(),
        )
        .unwrap_or_else(|err| panic!("evidence row for {} must build: {err}", vector.id));
        row.validate().expect("row must be honest");
        rows.push(row);
    }

    let status = rows.iter().all(|row| row.row_verdict);
    let report = SettlingGpuReport {
        schema: SettlingReportSchema::default(),
        scenario_id: "settling-sphere-v1".to_string(),
        candidate: SETTLING_GPU_CANDIDATE_DESCRIPTION.to_string(),
        units: flexpart_gpu::gpu::SettlingReportUnits::default(),
        comparison_policy: policy,
        rows,
        status,
    };
    report.validate().expect("report must validate");

    // Machine-readable comparison output records inputs, units, oracle/GPU
    // values, errors and verdicts.
    let out_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("target");
    std::fs::create_dir_all(&out_dir).expect("target dir must exist");
    let report_path = out_dir.join("settling-gpu-evidence.json");
    std::fs::write(
        &report_path,
        serde_json::to_string_pretty(&report).expect("report must serialize"),
    )
    .expect("report must write");
    report
        .require_paired_pass()
        .expect("report must prove paired pass");

    // Honest numerical failures remain serializable, but cannot pass the gate.
    let failed_row = build_settling_gpu_row(
        &ctx,
        "failure-diagnostic",
        "AERO",
        queries[0],
        gpu_values[0] * 1.1,
        report.rows[0].oracle_settling_velocity_m_s,
        policy,
        &revision,
        oracle_evidence.clone(),
    )
    .expect("failed numerical evidence builds");
    assert!(!failed_row.row_verdict);
    failed_row
        .validate()
        .expect("failed verdict is structurally honest");
    assert!(failed_row.gpu_evidence.require_paired_pass().is_err());
    let mut duplicate_report = report.clone();
    duplicate_report.rows.push(report.rows[0].clone());
    assert!(duplicate_report.validate().is_err());
    let mut invalid_row = report.rows[0].clone();
    invalid_row.temperature_k = f32::NAN;
    assert!(invalid_row.validate().is_err());

    // GPU execution proof: WGSL device path with adapter provenance.
    for row in &report.rows {
        assert_eq!(
            row.gpu_evidence.execution.calculation_path,
            GpuCalculationPath::WgslDevice
        );
        assert_eq!(
            row.gpu_evidence.execution.status,
            GpuExecutionStatus::Passed
        );
        assert!(
            row.gpu_evidence.execution.adapter.is_some(),
            "adapter provenance must be present"
        );
        assert_eq!(
            row.gpu_evidence.candidate.implementation_id,
            SETTLING_GPU_IMPLEMENTATION_ID
        );
        assert_eq!(
            row.gpu_evidence.candidate.shader_sha256,
            settling_shader_sha256()
        );
    }
    // Input binding proof: evidence input hashes must match dispatched inputs.
    for (row, query) in report.rows.iter().zip(queries.iter()) {
        let expected = settling_inputs_sha256(std::slice::from_ref(query)).expect("input hash");
        assert_eq!(row.gpu_evidence.candidate.input_sha256, expected);
    }
    assert_eq!(report.schema.id, SETTLING_GPU_REPORT_SCHEMA_ID);
    assert_eq!(report.schema.version, SETTLING_GPU_REPORT_SCHEMA_VERSION);
}

#[test]
fn test_settling_velocity_alone_does_not_remove_mass() {
    // The settling velocity kernel binds no particle mass buffer; settling
    // displaces particles vertically in #37 but never attenuates mass here.
    // This test proves mass invariance across a settling dispatch: particle
    // masses before and after computing velocities are bit-identical.
    let mut store = ParticleStore::with_capacity(2);
    let mut mass0 = [0.0; MAX_SPECIES];
    mass0[0] = 1.5;
    mass0[1] = 0.75;
    store
        .add(flexpart_gpu::particles::Particle::new(&ParticleInit {
            cell_x: 0,
            cell_y: 0,
            pos_x: 0.5,
            pos_y: 0.5,
            pos_z: 100.0,
            mass: mass0,
            release_point: 0,
            class: 0,
            time: 0,
        }))
        .expect("slot 0 available");
    let before: Vec<[f32; MAX_SPECIES]> = store
        .as_slice()
        .iter()
        .map(|particle| particle.mass)
        .collect();

    let ctx = match try_gpu_context() {
        Some(ctx) => ctx,
        None => return,
    };
    let particles = flexpart_gpu::gpu::ParticleBuffers::from_store(&ctx, &store);
    let kernel = SettlingVelocityKernel::new(&ctx).expect("settling kernel compiles");
    let carrier = SettlingCarrier {
        particle_density_kg_m3: 1000.0,
        diameter_um: 10.0,
    };
    let query = SettlingQuery::new(carrier, 293.15, 1.2).expect("query valid");
    let velocities = pollster::block_on(compute_settling_velocities_gpu(&ctx, &[query], &kernel))
        .expect("GPU settling succeeds");
    assert_eq!(velocities.len(), 1);
    assert!(velocities[0] < 0.0, "settling is downward");

    let downloaded =
        pollster::block_on(particles.download_particles(&ctx)).expect("particle readback");
    let after: Vec<[f32; MAX_SPECIES]> = downloaded.iter().map(|particle| particle.mass).collect();
    assert_eq!(before, after, "settling velocities must not alter mass");
}

#[test]
fn test_settling_direct_queries_are_rejected_before_upload() {
    let valid = SettlingQuery::new(
        SettlingCarrier {
            diameter_um: 10.0,
            particle_density_kg_m3: 1000.0,
        },
        293.15,
        1.2,
    )
    .expect("valid query");
    let invalid = [
        SettlingQuery {
            diameter_um: 0.0,
            ..valid
        },
        SettlingQuery {
            particle_density_kg_m3: f32::NAN,
            ..valid
        },
        SettlingQuery {
            temperature_k: 400.0,
            ..valid
        },
        SettlingQuery {
            air_density_kg_m3: 0.0,
            ..valid
        },
    ];
    for query in &invalid {
        assert!(query.validate().is_err());
        assert!(settling_inputs_sha256(&[*query]).is_err());
    }
    let Some(ctx) = try_gpu_context() else {
        return;
    };
    for query in &invalid {
        assert!(flexpart_gpu::gpu::create_settling_query_buffers(&ctx, &[*query]).is_err());
    }
}

#[test]
fn test_settling_evidence_rejects_dishonest_rows() {
    let row = flexpart_gpu::gpu::SettlingGpuRow {
        vector_id: "SETTLE-001".to_string(),
        species_name: "AERO".to_string(),
        diameter_um: 0.1,
        particle_density_kg_m3: 1000.0,
        temperature_k: 293.15,
        air_density_kg_m3: 1.2,
        gpu_settling_velocity_m_s: 0.001,
        oracle_settling_velocity_m_s: -0.001,
        comparison_policy: comparison_policy(),
        absolute_difference: 0.002,
        relative_difference: 2.0,
        row_verdict: true,
        gpu_evidence: flexpart_gpu::gpu::GpuCalculationEvidence {
            schema: flexpart_gpu::gpu::GpuEvidenceSchema::default(),
            case_id: "settling-gpu/SETTLE-001".to_string(),
            candidate: flexpart_gpu::gpu::GpuCandidateEvidence {
                implementation_id: SETTLING_GPU_IMPLEMENTATION_ID.to_string(),
                revision: "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".to_string(),
                shader_sha256: settling_shader_sha256(),
                input_sha256: "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"
                    .to_string(),
            },
            execution: flexpart_gpu::gpu::GpuExecutionEvidence {
                status: GpuExecutionStatus::Passed,
                calculation_path: GpuCalculationPath::WgslDevice,
                adapter: None,
                failure: None,
                skip_reason: None,
            },
            oracle: None,
            comparison: flexpart_gpu::gpu::ComparisonEvidence::not_evaluated(),
        },
    };
    assert!(matches!(
        row.validate(),
        Err(GpuEvidenceError::InvalidComparisonState(_))
    ));
}
