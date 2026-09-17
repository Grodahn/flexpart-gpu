//! Reference environment and ETEX independence tests (RISK-03.3G-01).
//!
//! These tests run without GPU, Fortran, or network access. They pin the
//! normative FLEXPART 11.1 oracle revision, prove that the bundled ETEX-1
//! source term and measurements are independent published data (never derived
//! from `flexpart-gpu` outputs), and enforce the fail-closed rule that a
//! candidate-derived reference must not back a real-dataset claim.

use std::path::PathBuf;

use flexpart_gpu::reference::ReferenceManifest;
use flexpart_gpu::validation::{EtexValidationError, EtexValidationFixture, EtexValidationHarness};

/// Full pinned oracle commit (FLEXPART v11.1).
const PINNED_COMMIT: &str = "c70586c2b7f5258850705325881c61f557ea9bd8";

fn repo_file(relative: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(relative)
}

fn read_repo_file(relative: &str) -> String {
    std::fs::read_to_string(repo_file(relative))
        .unwrap_or_else(|_| panic!("fixture file must exist: {relative}"))
}

#[test]
fn test_bundled_manifest_pins_flexpart_11_1_oracle() {
    let manifest = ReferenceManifest::bundled().expect("bundled manifest loads");
    assert_eq!(manifest.name, "FLEXPART");
    assert_eq!(manifest.version, "11.1");
    assert_eq!(manifest.pinned_tag, "v11.1");
    assert_eq!(manifest.pinned_commit, PINNED_COMMIT);
    assert!(manifest
        .upstream_repository
        .contains("gitlab.phaidra.org/flexpart/flexpart"));
}

#[test]
fn test_real_etex_release_matches_documented_source_term() {
    // Independent ETEX-1 source term: Monterfil release, 23 Oct 1994 16:00 UTC,
    // 340 kg PMCH over about 12 h. Values must match the published experiment,
    // not any flexpart-gpu output.
    let releases = read_repo_file("fixtures/etex/real/config/RELEASES");
    for expected in [
        "IDATE1  =       19941023",
        "ITIME1  =         160000",
        "IDATE2  =       19941024",
        "LON1    =         -2.000",
        "LAT1    =         48.058",
        "MASS    =   3.4000E+05",
        "ETEX1_PMCH",
    ] {
        assert!(
            releases.contains(expected),
            "real RELEASES must carry independent ETEX-1 source term, missing: {expected}"
        );
    }
}

#[test]
fn test_measurement_fixture_uses_independent_station_format() {
    let measurements = read_repo_file("fixtures/etex/data/meas-t1.txt");
    assert!(
        measurements.contains("year mn dy  shr  dur   lat     lon"),
        "measurement file must keep the independent station format header"
    );
    assert!(
        measurements.len() > 100_000,
        "measurement file must hold the full station record set"
    );
    let stations = read_repo_file("fixtures/etex/data/stations.txt");
    assert!(stations.contains("stn"), "station list must be present");
}

#[test]
fn test_provenance_documents_independence() {
    let provenance = read_repo_file("fixtures/etex/PROVENANCE.md");
    for expected in ["independent", "never", "not permitted", "monterfil", "1994"] {
        assert!(
            provenance.to_lowercase().contains(expected),
            "provenance must document independence, missing: {expected}"
        );
    }
}

#[test]
fn test_scaffold_fixture_stays_synthetic_without_dataset_claim() {
    let harness = EtexValidationHarness::load_from_json(&repo_file(
        "fixtures/etex/scaffold/validation_fixture.json",
    ))
    .expect("scaffold fixture loads");
    let reference = &harness.fixture().reference;
    assert!(
        reference.dataset_path.is_none(),
        "scaffold with derived reference must not claim a dataset"
    );
    reference.validate().expect("scaffold default stays valid");
}

#[test]
fn test_derived_reference_with_dataset_fails_closed_without_gpu() {
    let content = read_repo_file("fixtures/etex/scaffold/validation_fixture.json");
    let mut fixture: EtexValidationFixture =
        serde_json::from_str(&content).expect("scaffold fixture parses");
    fixture.reference.dataset_path = Some("fixtures/etex/data".to_string());
    let root = repo_file("fixtures/etex/scaffold");
    let harness = EtexValidationHarness::new(fixture, root);
    // The guard runs before any GPU work, so no adapter is required here.
    let error = harness
        .run_pipeline_synthetic()
        .expect_err("derived reference must not back a dataset claim");
    assert!(
        matches!(
            error,
            EtexValidationError::DerivedReferenceWithDataset { .. }
        ),
        "unexpected error: {error}"
    );
}
