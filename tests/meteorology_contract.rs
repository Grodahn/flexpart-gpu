use flexpart_gpu::meteorology::{
    ContractError, FieldId, Requirements, Snapshot, StorageOrder, SCHEMA_ID, SCHEMA_VERSION,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{fs, process::Command};

/// Pinned SHA-256 of the checked-in real-data fixture at commit time.
///
/// Recompute with `Get-FileHash`/`sha256sum` after regeneration and update
/// together with the provenance file. The test additionally recomputes the
/// digest from the embedded bytes so a stale pinned value (rather than a
/// changed file) fails loudly.
const REAL_DATA_FIXTURE_SHA256: &str =
    "a88723556201302298e17ab01eba2bedd830bc29f99548f50f52d27805f28b13";

/// SHA-256 produced by the pre-#106 Windows checkout mutation.
///
/// Git inserted `0x0d` before each of the fixture's 127 `0x0a` bytes, changing
/// its length from 37,544 to 37,671 bytes. This digest records the reproduced
/// failure without accepting the mutated representation as canonical.
const PRE_FIX_CRLF_FIXTURE_SHA256: &str =
    "b48447864fa2db4d61d8141a852099977dc78c729c9bb46d3b320c5f75dffec0";

/// SHA-256 of the source native model-level GRIB, as documented in the
/// provenance file and the original ETEX manifests.
const REAL_DATA_SOURCE_GRIB_SHA256: &str =
    "0e059410504dc3d1b2ba13628372a0be81054ff30a75017cf5b06cf9144b5023";

/// SHA-256 of the source ARCO-ERA5 surface archive (npz), as documented in the
/// provenance file.
const REAL_DATA_SOURCE_SURFACE_SHA256: &str =
    "88cd4dcb5e8ff89aeae6c065e3f2c2f75ad09c125f5d15290bfe7a56b059668d";

/// Selected valid time: 1994-10-23 15:00 UTC.
const REAL_DATA_EPOCH_SECONDS: i64 = 782_924_400;

/// Pinned FLEXPART oracle revision referenced by the provenance file.
const REAL_DATA_ORACLE_PINNED_COMMIT: &str = "c70586c2b7f5258850705325881c61f557ea9bd8";

#[test]
fn checked_in_synthetic_fixture_validates_and_roundtrips() {
    let source = include_str!("../fixtures/meteorology/synthetic-v1.json");
    let snapshot: Snapshot = serde_json::from_str(source).expect("parse canonical fixture");

    snapshot
        .validate(&Requirements::advection())
        .expect("checked-in canonical fixture must validate");

    assert_eq!(snapshot.provenance().schema_version, SCHEMA_VERSION);

    let encoded = serde_json::to_string(&snapshot).expect("serialize canonical fixture");
    let decoded: Snapshot = serde_json::from_str(&encoded).expect("roundtrip canonical fixture");
    assert_eq!(decoded, snapshot);
}

#[test]
fn checked_in_real_data_fixture_validates_and_roundtrips() {
    let source = include_str!("../fixtures/meteorology/era5-etex-native-v1.json");
    let snapshot: Snapshot = serde_json::from_str(source).expect("parse real-data fixture");

    snapshot
        .validate(&Requirements::real_data_native_levels())
        .expect("checked-in real-data fixture must validate its own requirement set");

    assert_eq!(snapshot.schema.id, SCHEMA_ID);
    assert_eq!(snapshot.schema.version, SCHEMA_VERSION);
    assert_eq!(snapshot.provenance().schema_version, SCHEMA_VERSION);

    // The native ERA5 snapshot carries exactly wind, temperature, humidity and
    // surface pressure: canonical 3-D pressure and upward-positive vertical
    // velocity are hybrid transforms owned by #30 and must fail closed here.
    assert_eq!(
        snapshot.validate(&Requirements::advection()),
        Err(ContractError::MissingRequiredField(
            FieldId::VerticalVelocity
        ))
    );

    // All fields must share one valid time and one explicit linearization.
    for field in &snapshot.fields {
        assert_eq!(
            field.time.valid_time_epoch_seconds, REAL_DATA_EPOCH_SECONDS,
            "field {:?} must be valid at the documented slice time",
            field.id
        );
        assert_eq!(
            field.storage_order,
            StorageOrder::XFastest,
            "field {:?} must use the canonical x-fastest layout",
            field.id
        );
    }

    // Native hybrid coefficients must remain at the 138 interfaces so #30 can
    // reconstruct interface pressure for the actual local surface pressure.
    let vertical = &snapshot.vertical_coordinate;
    let interfaces = vertical
        .interface_values
        .as_deref()
        .expect("interfaces present");
    let a = vertical
        .hybrid_a_interface_pa
        .as_deref()
        .expect("native interface A coefficients present");
    let b = vertical
        .hybrid_b_interface
        .as_deref()
        .expect("native interface B coefficients present");
    let reference_surface_pressure_pa = vertical
        .reference_surface_pressure_pa
        .expect("explicit hybrid reference surface pressure present");
    assert_eq!(a.len(), 138);
    assert_eq!(b.len(), 138);
    assert_eq!(interfaces.len(), 138);
    assert_eq!(a[0], 0.0);
    assert_eq!(b[0], 0.0);
    assert!(a[137].abs() <= 1.0e-3);
    assert!((b[137] - 1.0).abs() <= 1.0e-6);
    for index in 0..interfaces.len() {
        let reconstructed =
            a[index] as f64 + b[index] as f64 * reference_surface_pressure_pa as f64;
        assert!(
            (interfaces[index] as f64 - reconstructed).abs() <= 0.11,
            "interface {index} reference pressure must satisfy a+b*ps"
        );
    }

    // The half-level averaging convention documented in the provenance must
    // hold for every full level: P_full(k) = 0.5 * (P_half(k) + P_half(k+1)).
    assert_eq!(interfaces.len(), vertical.level_values.len() + 1);
    assert_eq!(interfaces[0], 0.0);
    assert_eq!(*interfaces.last().expect("non-empty interfaces"), 101_325.0);
    for (level, (value, pair)) in vertical
        .level_values
        .iter()
        .zip(interfaces.windows(2))
        .enumerate()
    {
        let averaged = 0.5 * (pair[0] as f64 + pair[1] as f64);
        assert!(
            (*value as f64 - averaged).abs() <= 0.05,
            "level {level} must be the half-level average ({value} vs {averaged})"
        );
    }

    let encoded = serde_json::to_string(&snapshot).expect("serialize canonical fixture");
    let decoded: Snapshot = serde_json::from_str(&encoded).expect("roundtrip canonical fixture");
    assert_eq!(decoded, snapshot);
}

#[test]
fn real_data_fixture_provenance_metadata_is_present_and_consistent() {
    let fixture_bytes = include_bytes!("../fixtures/meteorology/era5-etex-native-v1.json");
    let fixture_source = include_str!("../fixtures/meteorology/era5-etex-native-v1.json");
    let provenance_source =
        include_str!("../fixtures/meteorology/era5-etex-native-v1.provenance.json");

    let snapshot: Snapshot = serde_json::from_str(fixture_source).expect("parse real-data fixture");
    let provenance: FixtureProvenance = serde_json::from_str(provenance_source)
        .expect("real-data provenance metadata must be present and well-formed");

    // Canonical schema identity must be shared by artifact and provenance and
    // must match the snapshot itself (fail closed on any drift).
    assert_eq!(provenance.schema.id, SCHEMA_ID);
    assert_eq!(provenance.schema.version, SCHEMA_VERSION);
    assert_eq!(provenance.schema.id, snapshot.schema.id);
    assert_eq!(provenance.schema.version, snapshot.schema.version);

    // The artifact digest must be the pinned value and the digest computed from
    // the embedded bytes: a stale, absent or malformed hash fails loudly.
    assert_eq!(provenance.artifact.sha256, REAL_DATA_FIXTURE_SHA256);
    verify_exact_sha256(fixture_bytes, REAL_DATA_FIXTURE_SHA256)
        .expect("checked-in fixture must match exact-byte provenance");

    // The provenance must name the exact artifact and its size, and the
    // extraction script as the generation path.
    assert_eq!(
        provenance.artifact.path,
        "fixtures/meteorology/era5-etex-native-v1.json"
    );
    assert_eq!(provenance.artifact.bytes, fixture_bytes.len() as u64);
    let generator = provenance
        .artifact
        .generated_by
        .as_deref()
        .expect("generation script must be recorded");
    assert!(
        generator.ends_with("extract_era5_etex_native_v1.py"),
        "unexpected generation script: {generator}"
    );

    // Source lineage: the exact requested retrieval identity and the pinned
    // source digests must agree with the checked-in ETEX manifests.
    assert_eq!(provenance.source.request_identity.date, "1994-10-23");
    assert_eq!(provenance.source.request_identity.time, "15/18/21");
    assert_eq!(provenance.source.request_identity.levelist, "1/to/137");
    assert_eq!(provenance.source.request_identity.levtype, "ml");
    assert_eq!(
        provenance.source.request_identity.param,
        "130/131/132/133/135"
    );
    let grib = provenance
        .source
        .native_grib
        .as_ref()
        .expect("source model-level GRIB must be recorded");
    assert!(grib.path.ends_with("era5-19941023-151821-ml.grib"));
    assert_eq!(grib.bytes, 13_624_650);
    assert_eq!(grib.sha256, REAL_DATA_SOURCE_GRIB_SHA256);
    let surface = provenance
        .source
        .surface_archive
        .as_ref()
        .expect("source surface archive must be recorded");
    assert!(surface.path.ends_with("era5-surface-19941023-24.npz"));
    assert_eq!(surface.bytes, 531_362);
    assert_eq!(surface.sha256, REAL_DATA_SOURCE_SURFACE_SHA256);

    // Selected slice metadata must line up with the snapshot geometry and time.
    assert_eq!(provenance.slice.timestamp, "1994-10-23T15:00:00Z");
    assert_eq!(provenance.slice.epoch_seconds, REAL_DATA_EPOCH_SECONDS);
    assert_eq!(provenance.slice.horizontal.nx, snapshot.horizontal_grid.nx);
    assert_eq!(provenance.slice.horizontal.ny, snapshot.horizontal_grid.ny);
    assert_eq!(
        provenance.slice.horizontal.xlon0_deg,
        snapshot.horizontal_grid.xlon0_deg
    );
    assert_eq!(
        provenance.slice.horizontal.ylat0_deg,
        snapshot.horizontal_grid.ylat0_deg
    );
    assert_eq!(provenance.slice.horizontal.source_lon_indices, vec![24, 25]);
    assert_eq!(
        provenance.slice.horizontal.source_lat_desc_indices,
        vec![20, 19]
    );
    assert_eq!(provenance.slice.vertical.native_levels, 137);
    assert_eq!(
        provenance.slice.vertical.selected_levels,
        "1..137 (all native levels)"
    );

    // The provenance field list must document exactly the represented snapshot
    // fields (set membership, order-insensitive).
    let mut represented: Vec<String> = provenance.represented_fields.clone();
    represented.sort();
    let mut actual: Vec<String> = snapshot
        .fields
        .iter()
        .map(
            |field| match serde_json::to_value(field.id).expect("serialize field id") {
                serde_json::Value::String(name) => name,
                _ => panic!("field id must serialize to a JSON string"),
            },
        )
        .collect();
    actual.sort();
    assert_eq!(represented, actual);

    // The intentional #30 omissions and the normalization notes must be
    // documented verbatim, otherwise provenance is not actionable.
    assert!(!provenance.omitted_provider_fields.pressure_3d.is_empty());
    assert!(!provenance
        .omitted_provider_fields
        .omega_param_135
        .is_empty());
    assert!(!provenance
        .omitted_provider_fields
        .eta_velocity_param_77
        .is_empty());
    assert!(!provenance.normalization.is_empty());

    // The oracle reference must name the pinned FLEXPART 11.1 revision.
    assert_eq!(provenance.oracle_reference.name, "FLEXPART");
    assert_eq!(provenance.oracle_reference.version, "11.1");
    assert_eq!(
        provenance.oracle_reference.pinned_commit,
        REAL_DATA_ORACLE_PINNED_COMMIT
    );
}

#[test]
fn exact_byte_fixture_identity_rejects_crlf_and_content_mutation() {
    let fixture_bytes = include_bytes!("../fixtures/meteorology/era5-etex-native-v1.json");
    assert_eq!(fixture_bytes.len(), 37_544);
    assert_eq!(sha256_hex(fixture_bytes), REAL_DATA_FIXTURE_SHA256);
    assert!(
        !fixture_bytes.windows(2).any(|pair| pair == b"\r\n"),
        "the canonical fixture must be checked out with LF bytes"
    );

    let mut crlf_bytes = Vec::with_capacity(fixture_bytes.len() + 127);
    for byte in fixture_bytes {
        if *byte == b'\n' {
            crlf_bytes.push(b'\r');
        }
        crlf_bytes.push(*byte);
    }
    assert_eq!(crlf_bytes.len(), 37_671);
    assert_eq!(sha256_hex(&crlf_bytes), PRE_FIX_CRLF_FIXTURE_SHA256);
    assert!(verify_exact_sha256(&crlf_bytes, REAL_DATA_FIXTURE_SHA256).is_err());

    let mut content_mutation = fixture_bytes.to_vec();
    let first_digit = content_mutation
        .iter()
        .position(u8::is_ascii_digit)
        .expect("fixture contains numeric content");
    content_mutation[first_digit] = if content_mutation[first_digit] == b'9' {
        b'8'
    } else {
        content_mutation[first_digit] + 1
    };
    assert!(verify_exact_sha256(&content_mutation, REAL_DATA_FIXTURE_SHA256).is_err());

    assert_ne!(sha256_hex(b"arbitrary\n"), sha256_hex(b"arbitrary\r\n"));
}

#[test]
fn repository_policy_forces_lf_for_every_text_fixture_class() {
    let repository = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let safe_directory = format!("safe.directory={}", repository.display());
    let fixture_path = "fixtures/meteorology/era5-etex-native-v1.json";
    let tracked = Command::new("git")
        .args(["-c", &safe_directory, "ls-files", "-z", "fixtures"])
        .current_dir(repository)
        .output()
        .expect("git must be available to inspect the repository fixture policy");
    assert!(
        tracked.status.success(),
        "git ls-files failed: {}",
        String::from_utf8_lossy(&tracked.stderr)
    );

    let paths: Vec<&str> = tracked
        .stdout
        .split(|byte| *byte == 0)
        .filter(|path| !path.is_empty())
        .map(|path| std::str::from_utf8(path).expect("tracked fixture path must be UTF-8"))
        .collect();
    let text_paths: Vec<&str> = paths
        .iter()
        .copied()
        .filter(|path| !path.ends_with(".grib") && !path.ends_with(".npz"))
        .collect();
    assert!(!text_paths.is_empty());

    let mut command = Command::new("git");
    command
        .args([
            "-c",
            &safe_directory,
            "-c",
            "core.autocrlf=true",
            "check-attr",
            "text",
            "eol",
            "--",
        ])
        .args(&text_paths)
        .current_dir(repository);
    let attributes = command
        .output()
        .expect("git must be available to resolve fixture attributes");
    assert!(
        attributes.status.success(),
        "git check-attr failed: {}",
        String::from_utf8_lossy(&attributes.stderr)
    );
    let attributes = String::from_utf8(attributes.stdout).expect("attribute output must be UTF-8");
    for path in text_paths {
        assert!(
            attributes.contains(&format!("{path}: text: set")),
            "canonical text fixture lacks an explicit text policy: {path}"
        );
        assert!(
            attributes.contains(&format!("{path}: eol: lf")),
            "canonical text fixture lacks the repository-owned LF policy: {path}"
        );

        let bytes = fs::read(repository.join(path)).expect("read checked-out canonical fixture");
        assert!(
            !bytes.windows(2).any(|pair| pair == b"\r\n"),
            "canonical text fixture contains CRLF bytes: {path}"
        );
    }

    let simulated_checkout = tempfile::tempdir().expect("create simulated checkout directory");
    let checkout_prefix = format!("{}/", simulated_checkout.path().display()).replace('\\', "/");
    let checkout = Command::new("git")
        .args([
            "-c",
            &safe_directory,
            "-c",
            "core.autocrlf=true",
            "-c",
            "core.eol=crlf",
            "checkout-index",
            "--force",
            &format!("--prefix={checkout_prefix}"),
            "--",
            fixture_path,
        ])
        .current_dir(repository)
        .output()
        .expect("git must be available to simulate a Windows-style checkout");
    assert!(
        checkout.status.success(),
        "simulated checkout failed: {}",
        String::from_utf8_lossy(&checkout.stderr)
    );
    let checked_out_bytes =
        fs::read(simulated_checkout.path().join(fixture_path)).expect("read simulated checkout");
    assert_eq!(checked_out_bytes.len(), 37_544);
    assert_eq!(sha256_hex(&checked_out_bytes), REAL_DATA_FIXTURE_SHA256);
    assert!(!checked_out_bytes.windows(2).any(|pair| pair == b"\r\n"));
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn verify_exact_sha256(bytes: &[u8], expected: &str) -> Result<(), String> {
    let actual = sha256_hex(bytes);
    if actual == expected {
        Ok(())
    } else {
        Err(format!(
            "exact-byte SHA-256 mismatch: {actual} != {expected}"
        ))
    }
}

/// Minimal projection of the provenance JSON shape required by this fixture.
///
/// Unknown provenance keys are ignored so the record may evolve; every field
/// below is required and deserialization fails closed when it is absent or
/// malformed.
#[derive(Deserialize)]
struct FixtureProvenance {
    schema: SchemaMeta,
    artifact: ArtifactMeta,
    source: SourceMeta,
    slice: SliceMeta,
    represented_fields: Vec<String>,
    omitted_provider_fields: OmittedProviderFields,
    normalization: Vec<String>,
    oracle_reference: OracleReference,
}

#[derive(Deserialize)]
struct SchemaMeta {
    id: String,
    version: u32,
}

#[derive(Deserialize)]
struct ArtifactMeta {
    path: String,
    sha256: String,
    bytes: u64,
    #[serde(default)]
    generated_by: Option<String>,
}

#[derive(Deserialize)]
struct SourceMeta {
    request_identity: RequestIdentity,
    #[serde(default)]
    native_grib: Option<FileRef>,
    #[serde(default)]
    surface_archive: Option<FileRef>,
}

#[derive(Deserialize)]
struct RequestIdentity {
    date: String,
    time: String,
    levelist: String,
    levtype: String,
    param: String,
}

#[derive(Deserialize)]
struct FileRef {
    path: String,
    bytes: u64,
    sha256: String,
}

#[derive(Deserialize)]
struct OmittedProviderFields {
    pressure_3d: String,
    omega_param_135: String,
    eta_velocity_param_77: String,
}

#[derive(Deserialize)]
struct SliceMeta {
    timestamp: String,
    epoch_seconds: i64,
    horizontal: HorizontalSlice,
    vertical: VerticalSlice,
}

#[derive(Deserialize)]
struct HorizontalSlice {
    nx: usize,
    ny: usize,
    xlon0_deg: f64,
    ylat0_deg: f64,
    source_lon_indices: Vec<i64>,
    source_lat_desc_indices: Vec<i64>,
}

#[derive(Deserialize)]
struct VerticalSlice {
    native_levels: usize,
    selected_levels: String,
}

#[derive(Deserialize)]
struct OracleReference {
    name: String,
    version: String,
    pinned_commit: String,
}
