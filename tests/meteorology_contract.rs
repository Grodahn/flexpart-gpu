use flexpart_gpu::meteorology::{
    ContractError, FieldId, Requirements, Snapshot, SCHEMA_ID, SCHEMA_VERSION,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};

/// Pinned SHA-256 of the checked-in real-data fixture at commit time.
///
/// Recompute with `Get-FileHash`/`sha256sum` after regeneration and update
/// together with the provenance file. The test additionally recomputes the
/// digest from the embedded bytes so a stale pinned value (rather than a
/// changed file) fails loudly.
const REAL_DATA_FIXTURE_SHA256: &str =
    "215801bd6a2465cbecf6c15541d304ad8b4ae6efad3922f37a14d0f6462c7166";

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

    // All fields must share one valid time, matching the documented slice.
    for field in &snapshot.fields {
        assert_eq!(
            field.time.valid_time_epoch_seconds, REAL_DATA_EPOCH_SECONDS,
            "field {:?} must be valid at the documented slice time",
            field.id
        );
    }

    // The half-level averaging convention documented in the provenance must
    // hold for every full level: P_full(k) = 0.5 * (P_half(k) + P_half(k+1)).
    let vertical = &snapshot.vertical_coordinate;
    let interfaces = vertical
        .interface_values
        .as_deref()
        .expect("interfaces present");
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
    assert_eq!(sha256_hex(fixture_bytes), REAL_DATA_FIXTURE_SHA256);

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

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
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
