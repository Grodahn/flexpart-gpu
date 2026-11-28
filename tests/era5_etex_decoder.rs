//! Real bytes, independent ecCodes digests and canonical reference cross-checks.

#![cfg(feature = "eccodes")]

use flexpart_gpu::io::era5_etex::{
    decode_model_levels, load_snapshot, load_snapshot_with_surface, DecodeError,
    ModelLevelsDiagnostic, OmegaHandoff,
};
use flexpart_gpu::meteorology::vertical::{reconstruct_vertical_geometry, VerticalTransformError};
use flexpart_gpu::meteorology::{
    Axis, Calendar, ContractError, FieldId, Requirements, Snapshot, TemporalKind, Unit,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::Path;

const NATIVE: &str = "fixtures/etex/native-mini";
const DAY_ONE: &str = "era5-19941023-151821-ml.grib";
const DAY_TWO: &str = "era5-19941024-000306-ml.grib";
const LN_SP_ONE: &str = "era5-19941023-151821-lnsp.grib";
const LN_SP_TWO: &str = "era5-19941024-000306-lnsp.grib";
const SURFACE: &str = "era5-surface-19941023-24.npz";
const REFERENCE_TIME: i64 = 782_924_400;

#[derive(Deserialize)]
struct ValueEvidence {
    sources: Vec<SourceEvidence>,
}

#[derive(Deserialize)]
struct SourceEvidence {
    file: String,
    raw_sha256: String,
    pv_f32_le_sha256: String,
    volumes_by_epoch: BTreeMap<i64, BTreeMap<FieldId, String>>,
}

#[derive(Deserialize)]
struct OmegaEvidence {
    sources: Vec<OmegaSourceEvidence>,
}

#[derive(Deserialize)]
struct OmegaSourceEvidence {
    file: String,
    raw_sha256: String,
    request_record_sha256: String,
    by_epoch: BTreeMap<i64, OmegaVolumeEvidence>,
}

#[derive(Deserialize)]
struct OmegaVolumeEvidence {
    pv_f64_le_sha256: String,
    volume_f32_le_sha256: String,
    cell_count: usize,
}

#[derive(Deserialize)]
struct PressureEvidence {
    sources: Vec<PressureSourceEvidence>,
}

#[derive(Deserialize)]
struct PressureSourceEvidence {
    model_sha256: String,
    pressure_sha256: String,
    pressure_request_sha256: String,
    by_epoch: BTreeMap<i64, PressureGridEvidence>,
}

#[derive(Deserialize)]
struct PressureGridEvidence {
    pressure_f32_le_sha256: String,
}

#[derive(Deserialize)]
struct SnapshotEvidence {
    snapshot_sha256_by_epoch: BTreeMap<i64, String>,
}

#[test]
fn test_era5_etex_verified_snapshots_match_all_direct_grib_fields() {
    let models: ValueEvidence = serde_json::from_str(include_str!(
        "../fixtures/meteorology/era5-etex-native-v1.value-digests.json"
    ))
    .unwrap();
    let pressures: PressureEvidence = serde_json::from_str(include_str!(
        "../fixtures/meteorology/era5-etex-native-lnsp-v1.value-digests.json"
    ))
    .unwrap();
    let snapshots: SnapshotEvidence = serde_json::from_str(include_str!(
        "../fixtures/meteorology/era5-etex-native-lnsp-v1.snapshot-digests.json"
    ))
    .unwrap();
    assert_eq!(models.sources.len(), 2);
    assert_eq!(pressures.sources.len(), 2);
    assert_eq!(snapshots.snapshot_sha256_by_epoch.len(), 6);
    let base = Path::new(NATIVE);
    for ((model, pressure), pressure_file) in models
        .sources
        .iter()
        .zip(pressures.sources)
        .zip([LN_SP_ONE, LN_SP_TWO])
    {
        assert_eq!(model.raw_sha256, pressure.model_sha256);
        assert_eq!(pressure.by_epoch.len(), 3);
        for (&epoch, expected_pressure) in &pressure.by_epoch {
            let result = load_snapshot_with_surface(
                &base.join(&model.file),
                &base.join(pressure_file),
                epoch,
            )
            .unwrap();
            let snapshot = result.snapshot();
            snapshot
                .validate(&Requirements::real_data_native_levels())
                .unwrap();
            assert!(matches!(
                snapshot.validate(&Requirements::advection()),
                Err(ContractError::MissingRequiredField(
                    FieldId::VerticalVelocity
                ))
            ));
            assert!(matches!(
                reconstruct_vertical_geometry(snapshot),
                Err(VerticalTransformError::Contract(
                    ContractError::MissingRequiredField(
                        FieldId::Orography | FieldId::Temperature2m | FieldId::Dewpoint2m
                    )
                ))
            ));
            assert_eq!(snapshot.fields.len(), 5);
            assert_eq!(
                result.provenance()["model_levels"]["omega"]["handoff"],
                "REQUIRES_PREPROCESSING"
            );
            let vertical = &snapshot.vertical_coordinate;
            let a = vertical.hybrid_a_interface_pa.as_ref().unwrap();
            let b = vertical.hybrid_b_interface.as_ref().unwrap();
            assert_eq!(vertical.level_values.len(), 137);
            assert_eq!(vertical.interface_values.as_ref().unwrap().len(), 138);
            assert_eq!((a.len(), b.len()), (138, 138));
            assert_eq!(f32_digest(a.iter().chain(b)), model.pv_f32_le_sha256);
            for field in &snapshot.fields {
                let expected = if field.id == FieldId::SurfacePressure {
                    assert_eq!(field.shape, [65, 41]);
                    assert_eq!(field.axis_order, [Axis::X, Axis::Y]);
                    assert_eq!(field.unit, Unit::Pascal);
                    &expected_pressure.pressure_f32_le_sha256
                } else {
                    assert_eq!(field.shape, [65, 41, 137]);
                    &model.volumes_by_epoch[&epoch][&field.id]
                };
                assert_eq!(field.time.valid_time_epoch_seconds, epoch);
                assert_eq!(field.time.calendar, Calendar::Gregorian);
                assert_eq!(field.time.kind, TemporalKind::Instantaneous);
                assert_eq!(
                    &f32_digest(&field.values),
                    expected,
                    "{epoch}/{:?}",
                    field.id
                );
            }
            let payload = serde_json::to_vec(snapshot).unwrap();
            let digest = format!("{:x}", Sha256::digest(&payload));
            assert_eq!(digest, snapshots.snapshot_sha256_by_epoch[&epoch]);
            let provenance = result.provenance();
            assert_eq!(provenance["valid_time_epoch_seconds"], epoch);
            assert_eq!(provenance["payload"]["sha256"], digest);
            assert_eq!(provenance["payload"]["bytes"], payload.len());
            assert_eq!(
                provenance["model_levels"]["raw_source"]["verified_sha256"],
                model.raw_sha256
            );
            let source = &provenance["surface_pressure"];
            assert_eq!(source["request_record"]["sha256"], pressure.pressure_sha256);
            assert_eq!(
                source["request_record_sha256"],
                pressure.pressure_request_sha256
            );
        }
    }
}

fn f32_digest<'a>(values: impl IntoIterator<Item = &'a f32>) -> String {
    let mut hash = Sha256::new();
    for value in values {
        hash.update(value.to_le_bytes());
    }
    format!("{:x}", hash.finalize())
}

#[test]
fn test_era5_etex_surface_substitution_and_file_corruption_fail_closed() {
    let base = Path::new(NATIVE);
    let model = base.join(DAY_ONE);
    let load = |path: &Path| load_snapshot_with_surface(&model, path, REFERENCE_TIME);
    let scratch = tempfile::tempdir().unwrap();
    let corrupt_path = scratch.path().join(LN_SP_ONE);
    assert!(matches!(
        load(&corrupt_path),
        Err(DecodeError::SurfaceIo(_))
    ));
    let mut bytes = std::fs::read(base.join(LN_SP_ONE)).unwrap();
    bytes[100] ^= 1;
    std::fs::write(&corrupt_path, bytes).unwrap();
    for source in [base.join(SURFACE), base.join(LN_SP_TWO), corrupt_path] {
        assert!(matches!(
            load(&source),
            Err(DecodeError::SurfaceSourceIdentity)
        ));
    }
}

#[test]
fn test_era5_etex_omega_matches_every_direct_grib_value_and_blocks_handoff() {
    let evidence: OmegaEvidence = serde_json::from_str(include_str!(
        "../fixtures/meteorology/era5-etex-native-omega-v1.value-digests.json"
    ))
    .unwrap();
    let decision_bytes =
        include_bytes!("../fixtures/meteorology/era5-etex-native-omega-v1.decision.json");
    let decision: serde_json::Value = serde_json::from_slice(decision_bytes).unwrap();
    let decision_sha256 = format!("{:x}", Sha256::digest(decision_bytes));
    assert_eq!(evidence.sources.len(), 2);
    for (source, file) in evidence.sources.iter().zip([DAY_ONE, DAY_TWO]) {
        assert_eq!(source.file, file);
        assert_eq!(source.by_epoch.len(), 3);
        for (&epoch, expected) in &source.by_epoch {
            let model = Path::new(NATIVE).join(file);
            let diagnostic = decode_model_levels(&model, epoch).unwrap();
            let omega = diagnostic.omega_pa_s();
            assert_eq!(omega.len(), 65 * 41 * 137);
            assert_eq!(omega.len(), expected.cell_count);
            assert_eq!(f32_digest(omega), expected.volume_f32_le_sha256, "{epoch}");
            assert_eq!(
                diagnostic.omega_handoff(),
                OmegaHandoff::RequiresPreprocessing
            );
            let provenance = diagnostic.provenance();
            assert_eq!(provenance["omega"]["handoff"], decision["status"]);
            assert_eq!(provenance["omega"]["decision_sha256"], decision_sha256);
            assert_eq!(
                provenance["omega"]["volume_f32_le_sha256"],
                expected.volume_f32_le_sha256
            );
            assert_eq!(provenance["omega"]["epoch"], epoch);
            assert_eq!(
                provenance["grib"]["pv_f64_le_sha256"],
                expected.pv_f64_le_sha256
            );
            assert_eq!(
                provenance["raw_source"]["verified_sha256"],
                source.raw_sha256
            );
            assert_eq!(
                provenance["raw_source"]["request_record_sha256"],
                source.request_record_sha256
            );
            assert!(diagnostic
                .fields()
                .iter()
                .all(|field| field.id != FieldId::VerticalVelocity));
        }
    }
}

#[test]
fn test_era5_etex_all_times_match_independent_ec_codes_digests() {
    let evidence: ValueEvidence = serde_json::from_str(include_str!(
        "../fixtures/meteorology/era5-etex-native-v1.value-digests.json"
    ))
    .unwrap();
    assert_eq!(evidence.sources.len(), 2);
    for (source, file) in evidence.sources.iter().zip([DAY_ONE, DAY_TWO]) {
        assert_eq!(source.file, file);
        assert_eq!(source.volumes_by_epoch.len(), 3);
        for (&epoch, expected) in &source.volumes_by_epoch {
            // No canonical reference or independent digests are supplied to the decoder.
            let diagnostic = decode_model_levels(&Path::new(NATIVE).join(file), epoch).unwrap();
            let vertical = diagnostic.vertical_coordinate();
            let mut pv_hash = Sha256::new();
            for value in vertical
                .hybrid_a_interface_pa
                .iter()
                .flatten()
                .chain(vertical.hybrid_b_interface.iter().flatten())
            {
                pv_hash.update(value.to_le_bytes());
            }
            assert_eq!(format!("{:x}", pv_hash.finalize()), source.pv_f32_le_sha256);
            let fields = diagnostic.fields();
            assert_eq!(fields.len(), 4);
            assert_eq!(expected.len(), 4);
            for field in fields {
                let mut hash = Sha256::new();
                for value in &field.values {
                    hash.update(value.to_le_bytes());
                }
                assert_eq!(field.shape, [65, 41, 137]);
                assert_eq!(field.time.valid_time_epoch_seconds, epoch);
                assert_eq!(field.time.calendar, Calendar::Gregorian);
                assert_eq!(field.time.kind, TemporalKind::Instantaneous);
                assert_eq!(
                    format!("{:x}", hash.finalize()),
                    expected[&field.id],
                    "{epoch}/{:?}",
                    field.id
                );
            }
            let record = serde_json::to_value(&diagnostic).unwrap();
            assert_eq!(
                record["provenance"]["raw_source"]["verified_sha256"],
                source.raw_sha256
            );
            assert_eq!(
                record["provenance"]["raw_source"]["verified_bytes"],
                13_624_650
            );
            assert_eq!(
                record["provenance"]["grib"]["validated_message_count"],
                2055
            );
            assert_eq!(
                record["provenance"]["grib"]["levels"],
                serde_json::json!([1, 137])
            );
            assert!(
                record["provenance"]["toolchain"]["eccodes_api_version"]
                    .as_i64()
                    .unwrap()
                    >= 22_400
            );
            if epoch == REFERENCE_TIME {
                compare_canonical_reference(&diagnostic);
                assert!(serde_json::from_value::<Snapshot>(record.clone()).is_err());
                let payload: Snapshot =
                    serde_json::from_value(record["model_levels"].clone()).unwrap();
                assert!(matches!(
                    payload.validate(&Requirements::real_data_native_levels()),
                    Err(
                        flexpart_gpu::meteorology::ContractError::MissingRequiredField(
                            FieldId::SurfacePressure
                        )
                    )
                ));
                let bytes = serde_json::to_vec(&payload).unwrap();
                assert_eq!(record["provenance"]["payload"]["bytes"], bytes.len());
                assert_eq!(
                    record["provenance"]["payload"]["sha256"],
                    format!("{:x}", Sha256::digest(&bytes))
                );
            }
        }
    }
}

fn compare_canonical_reference(decoded: &ModelLevelsDiagnostic) {
    let reference: Snapshot = serde_json::from_str(include_str!(
        "../fixtures/meteorology/era5-etex-native-v1.json"
    ))
    .unwrap();
    reference
        .validate(&Requirements::real_data_native_levels())
        .unwrap();
    let grid = decoded.horizontal_grid();
    assert_eq!((grid.nx, grid.ny), (65, 41));
    let vertical = decoded.vertical_coordinate();
    let expected = &reference.vertical_coordinate;
    // The frozen #29 reference has small A differences from the raw GRIB PV.
    let actual_a = vertical.hybrid_a_interface_pa.as_ref().unwrap();
    let expected_a = expected.hybrid_a_interface_pa.as_ref().unwrap();
    assert_eq!(actual_a.len(), 138);
    assert_eq!(expected_a.len(), 138);
    for (index, (actual, expected)) in actual_a.iter().zip(expected_a).enumerate() {
        assert!(
            (actual - expected).abs() <= 0.000_25,
            "A[{index}]: {actual} vs {expected}"
        );
    }
    assert_eq!(vertical.hybrid_b_interface, expected.hybrid_b_interface);
    assert_eq!(vertical.level_values.len(), 137);
    assert_eq!(vertical.interface_values.as_ref().unwrap().len(), 138);
    for (actual, expected) in vertical
        .level_values
        .iter()
        .chain(vertical.interface_values.iter().flatten())
        .zip(
            expected
                .level_values
                .iter()
                .chain(expected.interface_values.iter().flatten()),
        )
    {
        assert!(
            actual.to_bits().abs_diff(expected.to_bits()) <= 1,
            "{actual} vs {expected}"
        );
    }

    for reference_field in reference
        .fields
        .iter()
        .filter(|field| field.id != FieldId::SurfacePressure)
    {
        let actual = decoded
            .fields()
            .iter()
            .find(|field| field.id == reference_field.id)
            .unwrap();
        assert_eq!(actual.unit, reference_field.unit);
        assert_eq!(actual.sign, reference_field.sign);
        assert_eq!(actual.time, reference_field.time);
        for z in 0..137 {
            for y in 0..2 {
                for x in 0..2 {
                    // The reference's 2x2 slice begins at (-2, 48) on the full grid.
                    let index = 24 + x + 65 * (20 + y + 41 * z);
                    let slice_index = x + 2 * (y + 2 * z);
                    assert!(
                        actual.values[index]
                            .to_bits()
                            .abs_diff(reference_field.values[slice_index].to_bits())
                            <= 1,
                        "{:?} at level {}, cell ({x}, {y})",
                        actual.id,
                        z + 1
                    );
                }
            }
        }
    }
}

#[test]
fn test_era5_etex_missing_wrong_or_mixed_source_rejected() {
    let base = Path::new(NATIVE);
    let model = base.join(DAY_ONE);
    let scratch = tempfile::tempdir().unwrap();
    let missing = scratch.path().join("missing.grib");
    assert!(matches!(
        decode_model_levels(&missing, REFERENCE_TIME),
        Err(DecodeError::Io(_))
    ));
    assert!(matches!(
        decode_model_levels(&model, REFERENCE_TIME + 1),
        Err(DecodeError::UnsupportedTime(_))
    ));
    assert!(matches!(
        decode_model_levels(&base.join(DAY_TWO), REFERENCE_TIME),
        Err(DecodeError::SourceIdentity)
    ));

    let mut corrupt = std::fs::read(&model).unwrap();
    corrupt[2000] ^= 1;
    let bad = scratch.path().join(DAY_ONE);
    std::fs::write(&bad, corrupt).unwrap();
    assert!(matches!(
        decode_model_levels(&bad, REFERENCE_TIME),
        Err(DecodeError::SourceIdentity)
    ));
    assert!(matches!(
        load_snapshot(&base.join(SURFACE), REFERENCE_TIME),
        Err(DecodeError::SourceIdentity)
    ));
    assert!(matches!(
        load_snapshot(&model, REFERENCE_TIME),
        Err(DecodeError::SurfacePressureUnavailable)
    ));

    // Even the real mixed-provenance files in the same directory cannot produce a Snapshot.
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_era5-etex-decode"))
        .arg("snapshot")
        .arg(&model)
        .arg(REFERENCE_TIME.to_string())
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("no verified matching surface pressure")
    );
    let mixed = std::process::Command::new(env!("CARGO_BIN_EXE_era5-etex-decode"))
        .arg("snapshot")
        .arg(&model)
        .arg(REFERENCE_TIME.to_string())
        .arg(base.join(SURFACE))
        .output()
        .unwrap();
    assert!(!mixed.status.success());
    assert!(mixed.stdout.is_empty());
}
