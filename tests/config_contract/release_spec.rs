//! Proof of the production release input boundary, with no particle or device execution.

use std::path::{Path, PathBuf};

use flexpart_gpu::config::{ReleaseConfig, SimulationConfig, SpeciesConfig};
use flexpart_gpu::coords::GridDomain;
use flexpart_gpu::release::{
    validate_release_bindings, ReleaseConversion, ReleaseGeometry, ReleaseHeight, ReleaseHeightRef,
    ReleaseProfile, ReleaseQuantity, ReleaseSource, ReleaseSpec, ReleaseTime,
};
use flexpart_gpu::simulation::{ForwardTimeLoopConfig, ForwardTimeLoopDriver};
use flexpart_gpu::validation::case::ValidationCaseManifest;
use serde_json::json;

const POINT: &str = "NAME=point,START=20240229010000,END=20240229010000,LON=10,LAT=50,Z1=100,Z2=100,VERTICAL_REF=agl,MASS_KG=8,PARTICLES=4,SPECIES=B";
const WINDOW: &str = "ID=window,START_TIME=20240229,END_TIME=2024-02-29T01:30:00Z,XPOINT1=9,XLON2=11,YPOINT1=49,YLAT2=51,Z_BOTTOM=-5,Z_TOP=120,HEIGHT_REF=asl,MASS_SPECIES='2,1.2345678901234567',MASS_UNIT=kg,NPART=17,SPECIES_NAMES='B,A',SAMPLING_SEED=72";

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn species() -> Vec<SpeciesConfig> {
    let directory = tempfile::tempdir().unwrap();
    [("SPECIES_040", "A"), ("SPECIES_024", "B")]
        .into_iter()
        .map(|(file, name)| {
            let path = directory.path().join(file);
            std::fs::write(&path, format!("NAME={name}")).unwrap();
            SpeciesConfig::from_file(&path).unwrap()
        })
        .collect()
}

fn releases(text: &str, species: &[SpeciesConfig]) -> Vec<ReleaseSpec> {
    let declarations = ReleaseConfig::parse_many(text, Path::new("RELEASES")).unwrap();
    ReleaseConfig::normalize_forward_mass(&declarations, species).unwrap()
}

#[test]
fn test_release_spec_project_aliases_overlap_and_roundtrip() {
    let species = species();
    let input = format!("&RELEASE {POINT} /\n&RELEASE {WINDOW} /");
    let normalized = releases(&input, &species);
    assert_eq!(normalized, releases(&input, &species));
    let [instant, window] = normalized.as_slice() else {
        panic!("expected two releases")
    };
    assert!(matches!(instant.geometry, ReleaseGeometry::Point { .. }));
    assert!(matches!(
        instant.height,
        ReleaseHeight::Fixed {
            reference: ReleaseHeightRef::Agl,
            ..
        }
    ));
    assert_eq!(instant.inventories[0].mass_slot, 1);
    assert_eq!(
        instant.inventories[0].quantity,
        ReleaseQuantity::Mass { kg: 8.0 }
    );
    let ReleaseProfile::Instant { at } = instant.profile else {
        panic!("expected instant")
    };
    let ReleaseProfile::UniformWindow { start, end } = window.profile else {
        panic!("expected window")
    };
    assert!(start < at && at < end);
    assert_eq!(window.geometry.bounds(), (9.0, 11.0, 49.0, 51.0));
    assert_eq!(window.height.bounds(), (ReleaseHeightRef::Asl, -5.0, 120.0));
    assert_eq!(window.sampling_seed, Some(72));
    assert_eq!(
        (
            window.inventories[0].species_id.as_str(),
            window.inventories[0].mass_slot
        ),
        ("A", 0)
    );
    assert_eq!(
        (
            window.inventories[1].species_id.as_str(),
            window.inventories[1].mass_slot
        ),
        ("B", 1)
    );
    let json = serde_json::to_string(&normalized).unwrap();
    assert_eq!(
        serde_json::from_str::<Vec<ReleaseSpec>>(&json).unwrap(),
        normalized
    );
    assert_eq!(
        serde_json::to_string(&releases(&input, &species)).unwrap(),
        json
    );
    assert!(!json.contains("\"raw\""));
}

#[test]
fn test_release_spec_native_species_numbers_and_explicit_unit_conversion() {
    let text = "\
&RELEASES_CTRL NSPEC=2, SPECNUM_REL=24, 40 /
&RELEASE IDATE1=20240101, ITIME1=0, IDATE2=20240101, ITIME2=0,
LON1=10,LON2=10,LAT1=50,LAT2=50,Z1=100,Z2=100,ZKIND=1,
MASS=3.4D5, 1024.125, PARTS=12,COMMENT='point ! # / &RELEASE' /
";
    let result = releases(text, &species());
    assert_eq!(result[0].id, "point ! # / &RELEASE");
    assert_eq!(result[0].inventories[0].species_id, "A");
    assert_eq!(
        result[0].inventories[0].quantity,
        ReleaseQuantity::Mass { kg: 1024.125 }
    );
    assert_eq!(
        result[0].inventories[1].quantity,
        ReleaseQuantity::Mass { kg: 340000.0 }
    );
    assert_eq!(result[0].inventories[1].conversion, None);
    let converted = releases(
        &POINT.replace("MASS_KG=8", "MASS=340000,MASS_UNIT=g"),
        &species(),
    );
    assert_eq!(
        converted[0].inventories[0].quantity,
        ReleaseQuantity::Mass { kg: 340.0 }
    );
    assert_eq!(
        converted[0].inventories[0].conversion,
        Some(ReleaseConversion::DeclaredGramsToKgV1 { input_g: 340000.0 })
    );
    assert_eq!(
        serde_json::from_str::<Vec<ReleaseSpec>>(&serde_json::to_string(&result).unwrap()).unwrap(),
        result
    );

    let root = repo().join("fixtures/corpus/fortran/ADV-ANA-001");
    let declarations = ReleaseConfig::load_many_from_file(&root.join("RELEASES")).unwrap();
    let species = [SpeciesConfig::from_file(&root.join("SPECIES/SPECIES_024")).unwrap()];
    let result = ReleaseConfig::normalize_forward_mass(&declarations, &species).unwrap();
    assert_eq!(
        result[0].inventories[0].quantity,
        ReleaseQuantity::Mass { kg: 1_024_000.0 }
    );
}

#[test]
fn test_release_spec_native_control_rejects_cardinality_and_duplicate_numbers() {
    for header in [
        "NSPEC=2,SPECNUM_REL=24,24",
        "NSPEC=2,SPECNUM_REL=24",
        "NSPEC=5,SPECNUM_REL=24,25,26,27,28",
        "NSPEC=2,SPECNUM_REL=24,40,SPECNUM_REL=24,40",
    ] {
        let input = format!("&RELEASES_CTRL {header} /");
        let error = ReleaseConfig::parse_many(&input, Path::new("RELEASES")).unwrap_err();
        assert!(error
            .to_string()
            .to_ascii_lowercase()
            .contains("specnum_rel"));
    }
}

#[test]
fn test_release_spec_legacy_species_mapping_requires_unambiguous_table() {
    let species = species();
    let implicit = POINT.replace(",SPECIES=B", "");
    let parsed = ReleaseConfig::parse_many(&implicit, Path::new("RELEASES")).unwrap();
    assert!(ReleaseConfig::normalize_forward_mass(&parsed, &species).is_err());
    let single = ReleaseConfig::normalize_forward_mass(&parsed, &species[1..]).unwrap();
    assert_eq!(single[0].inventories[0].species_id, "B");
    let vector = implicit.replace("MASS_KG=8", "MASS_SPECIES='0,8'");
    let ordered = releases(&vector, &species);
    assert_eq!(
        ordered[0].inventories[1].quantity,
        ReleaseQuantity::Mass { kg: 8.0 }
    );
    assert!(
        validate_release_bindings(&ordered, &[species[1].clone(), species[0].clone()]).is_err()
    );

    let repeated_species = vec![species[0].clone(); 5];
    assert!(ReleaseConfig::normalize_forward_mass(&parsed, &repeated_species).is_err());
    let repeated_name = [species[0].clone(), species[0].clone()];
    assert!(ReleaseConfig::normalize_forward_mass(&parsed, &repeated_name).is_err());
}

#[test]
fn test_release_spec_input_rejects_conflicts_and_missing_semantics() {
    let species = species();
    for (old, new) in [
        ("20240229010000", "20230229010000"),
        ("20240229010000", "2024-02-29T01:00:00"),
        ("20240229010000", "2024-02-29T01:00:00+01:00"),
        ("END=20240229010000", "END=20240228010000"),
        (
            "END=20240229010000",
            "END=20240229010000,PROFILE=uniform_window",
        ),
        ("END=20240229010000", "END=20240229010000,PROFILE=arbitrary"),
        ("LON=10", "LON=NaN"),
        ("LON=10", "LON=181"),
        ("LAT=50", "LAT=-91"),
        ("LAT=50", "LAT=50,LAT=50"),
        ("LAT=50", "LAT=50,LATITUDE=50"),
        ("Z2=100", "Z2=0"),
        ("Z1=100", "Z1=inf"),
        ("Z1=100", "Z1=-1"),
        (",Z1=100", ""),
        (",VERTICAL_REF=agl", ""),
        ("VERTICAL_REF=agl", "VERTICAL_REF=agl,ZKIND=1"),
        ("VERTICAL_REF=agl", "ZKIND=3"),
        ("MASS_KG=8", "MASS_KG=-1"),
        ("MASS_KG=8", "MASS_KG=inf"),
        ("MASS_KG=8", "MASS_KG=-1e-999"),
        ("MASS_KG=8", "MASS_KG=0"),
        ("MASS_KG=8", "MASS_KG=8,MASS_UNIT=g"),
        ("MASS_KG=8", "QUANTITY=8"),
        ("MASS_KG=8", "ACTIVITY_BQ=8"),
        ("MASS_KG=8", "MASS_KG=8,ACTIVITY_BQ=8"),
        ("MASS_KG=8", "MASS_KG=8,MASS_SPECIES='1 2'"),
        ("MASS_KG=8", "MASS_SPECIES='1, 2, 3, 4, 5'"),
        ("PARTICLES=4", "PARTICLES=0"),
        (",PARTICLES=4", ""),
        ("SPECIES=B", "SPECIES=unknown"),
        (
            "MASS_KG=8,PARTICLES=4,SPECIES=B",
            "MASS_SPECIES='1 2',PARTICLES=4,SPECIES='B,b'",
        ),
        (
            "LON=10,LAT=50",
            "LON_MIN=11,LON_MAX=10,LAT_MIN=45,LAT_MAX=50",
        ),
        ("LON=10,LAT=50", "LON_MIN=10,LON_MAX=11,LAT_MIN=45"),
        ("LON=10,LAT=50", "LON=10,LAT=50,LON_MIN=0"),
    ] {
        let text = POINT.replacen(old, new, 1);
        let result = ReleaseConfig::parse_many(&text, Path::new("RELEASES"))
            .and_then(|input| ReleaseConfig::normalize_forward_mass(&input, &species));
        assert!(result.is_err(), "accepted {text}");
    }
    for text in [
        format!("{POINT}\n{POINT}"),
        format!("&RELEASE {POINT} /\n&IGNORED /"),
        format!("&RELEASE {POINT} / trailing"),
    ] {
        let result = ReleaseConfig::parse_many(&text, Path::new("RELEASES"))
            .and_then(|input| ReleaseConfig::normalize_forward_mass(&input, &species));
        assert!(result.is_err(), "accepted {text}");
    }
    assert!(ReleaseConfig::parse_many(POINT, Path::new("")).is_err());
}

#[test]
fn test_release_spec_canonical_document_validates_version_enums_and_provenance() {
    let mut value = serde_json::to_value(&releases(POINT, &species())[0]).unwrap();
    for (path, replacement) in [
        ("/schema_version", json!(2)),
        ("/release/profile/kind", json!("constant")),
        ("/release/profile/at", json!(i64::MAX)),
        ("/release/geometry/kind", json!("polygon")),
        ("/release/horizontal_ref", json!("local_cartesian")),
        ("/release/height/kind", json!("gaussian")),
        ("/release/height/reference", json!("unknown")),
        ("/release/source/path", json!("")),
        ("/release/source/sha256", json!("broken")),
        ("/release/inventories/0/mass_slot", json!(4)),
        (
            "/release/inventories/0/quantity",
            json!({"kind":"mass","value":8}),
        ),
    ] {
        let mut rejected = value.clone();
        *rejected
            .pointer_mut(path)
            .unwrap_or_else(|| panic!("fixture lacks {path}")) = replacement;
        assert!(
            serde_json::from_value::<ReleaseSpec>(rejected).is_err(),
            "accepted {path}"
        );
    }
    value["release"]["inventories"][0]["quantity"] = json!({"kind":"activity","bq":3.0});
    assert!(serde_json::from_value::<ReleaseSpec>(value.clone()).is_err());
    value["release"]["inventories"][0]["quantity"]["reference_time"] =
        json!(ReleaseTime::parse_utc("2024-02-29T01:00:00Z")
            .unwrap()
            .unix_seconds());
    let activity = serde_json::from_value::<ReleaseSpec>(value).unwrap();
    assert!(matches!(
        activity.inventories[0].quantity,
        ReleaseQuantity::Activity { .. }
    ));
    assert_eq!(
        serde_json::from_value::<ReleaseSpec>(serde_json::to_value(&activity).unwrap()).unwrap(),
        activity
    );
    assert!(forward_error(activity, &species()).contains("activity cannot be injected as mass"));
}

fn forward_error(spec: ReleaseSpec, species: &[SpeciesConfig]) -> String {
    let grid = GridDomain {
        xlon0: 0.0,
        ylat0: 0.0,
        dx: 1.0,
        dy: 1.0,
        nx: 40,
        ny: 80,
    };
    match pollster::block_on(ForwardTimeLoopDriver::new(
        ForwardTimeLoopConfig::default(),
        &[spec],
        species,
        grid,
        1,
    )) {
        Ok(_) => panic!("unsupported release reached the driver"),
        Err(error) => error.to_string(),
    }
}

#[test]
fn test_release_spec_forward_rejects_invalid_lane_and_unsupported_asl_before_device() {
    let species = species();
    let mut specs = releases(&format!("{POINT}\n{WINDOW}"), &species);
    let window = specs.pop().unwrap();
    assert!(forward_error(window, &species).contains("requires AGL"));
    let point = specs.pop().unwrap();
    assert!(forward_error(point, &species[..1]).contains("does not bind"));
}

#[test]
fn test_release_spec_validation_case_adapter_preserves_declared_fields() {
    let root = repo();
    let manifest = ValidationCaseManifest::load_from_file(
        &root.join("fixtures/corpus/cases/ADV-ANA-001.json"),
    )
    .unwrap();
    let mut species = species();
    species[1] = SpeciesConfig::from_file(
        &root.join("fixtures/corpus/fortran/ADV-ANA-001/SPECIES/SPECIES_024"),
    )
    .unwrap();
    let spec = manifest.normalize_release(&species).unwrap();
    assert_eq!(spec.id, manifest.case_id);
    assert!(matches!(
        spec.source,
        ReleaseSource::ValidationCaseV2 { .. }
    ));
    assert_eq!(
        spec.profile,
        ReleaseProfile::Instant {
            at: ReleaseTime::parse_utc("20240101000000").unwrap()
        }
    );
    assert_eq!(spec.geometry.bounds(), (10.0, 10.0, 50.0, 50.0));
    assert_eq!(spec.height.bounds(), (ReleaseHeightRef::Agl, 100.0, 100.0));
    assert_eq!(
        spec.particle_count,
        u64::from(manifest.release.particle_count)
    );
    assert_eq!(spec.inventories[0].mass_slot, 1);
    assert_eq!(
        spec.inventories[0].quantity,
        ReleaseQuantity::Mass { kg: 1024.0 }
    );
    assert!(manifest.normalize_release(&species[..1]).is_err());
}

#[test]
fn test_release_spec_config_loading_rejects_unsupported_command_source_mode() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::write(
        root.join("COMMAND"),
        "&COMMAND START=20240101,END=20240102,IND_SOURCE=2 /",
    )
    .unwrap();
    std::fs::write(root.join("RELEASES"), POINT).unwrap();
    std::fs::write(
        root.join("OUTGRID"),
        "LON_MIN=0,LON_MAX=20,LAT_MIN=40,LAT_MAX=60,NX=20,NY=20,NZ=1,DZ=100",
    )
    .unwrap();
    std::fs::create_dir(root.join("SPECIES")).unwrap();
    std::fs::write(root.join("SPECIES/B.spec"), "NAME=B").unwrap();
    assert!(SimulationConfig::load(root)
        .unwrap_err()
        .to_string()
        .contains("ind_source"));
}
