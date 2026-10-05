//! Freeze public configuration behavior before the issue #129 structural split.

use std::collections::BTreeMap;
use std::path::Path;

use flexpart_gpu::config::{
    CommandConfig, ConfigError, OutputGridConfig, ReleaseConfig, SimulationConfig, SpeciesConfig,
    SpeciesKind,
};
use serde::{de::DeserializeOwned, Serialize};

const COMMAND: &str = "&COMMAND IBDATE=20240101, IEDATE=20240102 /";
const RELEASE: &str = "START=20240101, END=20240102, LON=7, LAT=46";
const GRID: &str = "LON_MIN=5,LON_MAX=15,LAT_MIN=40,LAT_MAX=50,NX=100,NY=50,NZ=1,DZ=100";

fn record<T: Serialize + DeserializeOwned>(
    records: &mut BTreeMap<String, String>,
    label: &str,
    result: Result<T, ConfigError>,
) {
    match result {
        Ok(value) => {
            let compact = serde_json::to_string(&value).expect("compact configuration");
            let pretty = serde_json::to_string_pretty(&value).expect("pretty configuration");
            let decoded: T = serde_json::from_str(&compact).expect("round trip");
            assert_eq!(
                serde_json::to_string(&decoded).expect("reserialize"),
                compact
            );
            records.insert(format!("{label}/compact"), compact);
            records.insert(format!("{label}/pretty"), pretty);
        }
        Err(error) => {
            records.insert(format!("{label}/error"), error.to_string());
        }
    }
}

fn simulation() -> SimulationConfig {
    let directory = tempfile::tempdir().expect("species directory");
    std::fs::write(directory.path().join("tracer.spec"), "NAME=tracer").expect("species");
    SimulationConfig {
        version: None,
        command: CommandConfig::parse(COMMAND, Path::new("COMMAND")).expect("command"),
        releases: ReleaseConfig::parse_many(RELEASE, Path::new("RELEASES")).expect("release"),
        outgrid: OutputGridConfig::parse(GRID, Path::new("OUTGRID")).expect("grid"),
        species: SpeciesConfig::load_dir(directory.path()).expect("species"),
    }
}

#[test]
fn test_config_public_facade_matches_pre_decomposition_behavior() {
    let mut records = BTreeMap::new();
    for (label, input) in [
        ("defaults", COMMAND.to_string()),
        ("explicit", "&COMMAND START_TIME=2024-01-01T12:34:56,END_TIME=20240102123456,OUTPUT_INTERVAL_SECONDS=3600,SYNC_INTERVAL_SECONDS=900 /".into()),
        ("aliases", "&command ibdatetime=2024010112,iedatetime=202401021234,outstep=60,lsynctime=30 /".into()),
        ("precedence", "&COMMAND START_TIME=20240101,START=20250101,END=20240102,LOUTSTEP=60,OUTSTEP=0,EXTRA='x,!#/' /".into()),
        ("duplicate", "&COMMAND START=20240101,START=20240102,END=20240103 /".into()),
        ("first_section", format!("{COMMAND}\n&COMMAND START=20250101,END=20240101 /")),
        ("comments", "# header\n&COMMAND\nSTART=20240101, ! start\nEND=20240102\n/".into()),
        ("missing_section", "START=20240101,END=20240102".into()),
        ("missing_terminator", "&COMMAND START=20240101".into()),
        ("missing_key", "&COMMAND END=20240101 /".into()),
        ("empty_value", "&COMMAND START=,END=20240102 /".into()),
        ("missing_assignment", "&COMMAND START /".into()),
        ("reversed", "&COMMAND START=20240102,END=20240101 /".into()),
        ("zero_output", "&COMMAND START=20240101,END=20240102,LOUTSTEP=0 /".into()),
        ("zero_sync", "&COMMAND START=20240101,END=20240102,LSYNCTIME=0 /".into()),
        ("negative_interval", "&COMMAND START=20240101,END=20240102,LOUTSTEP=-1 /".into()),
    ] {
        record(&mut records, &format!("command/{label}"), CommandConfig::parse(&input, Path::new("COMMAND")));
    }
    for timestamp in [
        "20240101",
        "2024010112",
        "202401011234",
        "20240101123456",
        "2024010",
        "00000101",
        "20241301",
        "20240132",
        "2024010124",
        "202401011260",
        "20240101123460",
    ] {
        record(
            &mut records,
            &format!("timestamp/{timestamp}"),
            CommandConfig::parse(
                &format!("&COMMAND START={timestamp},END=99991231 /"),
                Path::new("COMMAND"),
            ),
        );
    }
    for (label, input) in [
        ("defaults", RELEASE.to_string()),
        ("height_default", format!("{RELEASE},Z1=20")),
        ("aliases", "RELEASE_NAME=stack,START_TIME=20240101,END_TIME=20240102,LONGITUDE=7,LATITUDE=46,Z_BOTTOM=10,Z_TOP=20,XMASS=2,NPART=5,SPECIES_MASS='1, 2'".into()),
        ("explicit", format!("{RELEASE},NAME=stack,Z_MIN=10,Z_MAX=20,MASS_KG=2,PARTICLE_COUNT=5,MASS_SPECIES='1 2'")),
        ("precedence", format!("{RELEASE},NAME=first,ID=second,Z_MIN=10,Z1=99,MASS_KG=2,MASS=0,PARTICLE_COUNT=5,NPART=0")),
        ("blocks", format!("&RELEASE {RELEASE} /\n&RELEASE {RELEASE} /")),
        ("lines", format!("{RELEASE}\n# comment\n{RELEASE}")),
        ("empty", "".into()),
        ("reversed", "START=20240102,END=20240101,LON=7,LAT=46".into()),
        ("bad_lon", "START=20240101,END=20240102,LON=181,LAT=46".into()),
        ("bad_lat", "START=20240101,END=20240102,LON=7,LAT=91".into()),
        ("bad_height", format!("{RELEASE},Z1=10,Z2=0")),
        ("zero_mass", format!("{RELEASE},MASS=0")),
        ("zero_particles", format!("{RELEASE},PARTICLES=0")),
        ("too_many_masses", format!("{RELEASE},MASS_SPECIES='1,2,3,4,5'")),
        ("negative_masses", format!("{RELEASE},MASS_SPECIES='1,-1'")),
        ("nan_masses", format!("{RELEASE},MASS_SPECIES='NaN'")),
        ("zero_masses", format!("{RELEASE},MASS_SPECIES='0,0'")),
        ("invalid_masses", format!("{RELEASE},MASS_SPECIES='abc'")),
    ] {
        record(&mut records, &format!("release/{label}"), ReleaseConfig::parse_many(&input, Path::new("RELEASES")));
    }
    for (label, input) in [
        ("defaults", GRID.to_string()),
        ("namelist", format!("&OUTGRID {GRID} /")),
        (
            "aliases",
            "XLON0=5,XLON1=15,YLAT0=40,YLAT1=50,NX=100,NY=50,NZ=1,XRES=0.1,YRES=0.2,ZRES=100"
                .into(),
        ),
        (
            "short_aliases",
            "XMIN=5,XMAX=15,YMIN=40,YMAX=50,NX=100,NY=50,NZ=1,DZ=100".into(),
        ),
        ("precedence", format!("{GRID},DX=0.1,XRES=0,DY=0.2,YRES=0")),
        ("missing_dz", GRID.replace(",DZ=100", "")),
        ("bad_bounds", GRID.replace("LON_MAX=15", "LON_MAX=5")),
        ("bad_range", GRID.replace("LON_MIN=5", "LON_MIN=-181")),
        ("zero_dimension", GRID.replace("NX=100", "NX=0")),
        ("zero_spacing", format!("{GRID},DX=0")),
        ("invalid_dimension", GRID.replace("NX=100", "NX=oops")),
    ] {
        record(
            &mut records,
            &format!("grid/{label}"),
            OutputGridConfig::parse(&input, Path::new("OUTGRID")),
        );
    }
    for (label, input) in [
        ("defaults", "UNKNOWN=retained"),
        ("versioned", "NAME=tracer,VERSION=1"),
        ("legacy_namelist", "&SPECIES NAME=tracer /"),
        ("oracle_namelist", "&SPECIES_PARAMS PSPECIES=tracer /"),
        ("section_precedence", "&SPECIES NAME=legacy /\n&SPECIES_PARAMS PSPECIES=oracle /"),
        ("aliases", "SPECNAME=gas,SPECIES_VERSION=1,MOL_WEIGHT=29,VDEP=0.025,DECAY=0.0,F0=0"),
        ("gas", "PSPECIES=gas,PWEIGHTMOLAR=29,PDRYVEL=2.5,PDECAY=453168,PWETA_GAS=0.001,PWETB_GAS=0.8,PHENRY=1,PRELDIFF=0.5,PF0=0"),
        ("aerosol", "PSPECIES=aero,PDENSITY=1000,PDIA=0.00005,PDSIGMA=3.3,PCRAIN_AERO=1,PCSNOW_AERO=1,PCCN_AERO=0.9,PIN_AERO=0.1"),
        ("legacy_aerosol", "NAME=aero,DENSITY=1000,DIAMETER_UM=50,DSIGMA=3.3,CRAIN_AERO=1,CSNOW_AERO=1,CCN_AERO=0.9,IN_AERO=0.1"),
        ("sentinels", "PDECAY=-9.9,PDRYVEL=-9.9,PDIA=0,PDSIGMA=0,PDENSITY=-9.9,PWETA_GAS=-9.9,PWETB_GAS=-9.9,PCRAIN_AERO=-9.9,PCSNOW_AERO=-9.9,PCCN_AERO=-9.9,PIN_AERO=-9.9,PRELDIFF=-9.9,PHENRY=-9.9,PF0=-9,PWEIGHTMOLAR=-9.9"),
        ("precedence", "NAME=first,PSPECIES=second,PDRYVEL=0,VDEP=2,PDECAY=0,DECAY=2,PDIA=0,DIAMETER_UM=1,PF0=0,F0=2"),
        ("pdquer", "PDQUER=0.00005,PDSIGMA=3.3"),
        ("legacy_zero", "DRY_DEPOSITION_VELOCITY=0,DECAY_CONSTANT=0,PF0=0"),
        ("nan_positive_only", "PWEIGHTMOLAR=NaN,PDIA=NaN,PF0=NaN"),
        ("nan_dry", "PDRYVEL=NaN"),
        ("nan_decay", "PDECAY=NaN"),
        ("overflow_decay", "PDECAY=1e-309"),
        ("negative_legacy_decay", "DECAY=-1"),
        ("negative_legacy_dry", "VDEP=-1"),
        ("gas_particle", "PRELDIFF=0.5,PDENSITY=1000,PDIA=0.00001,PDSIGMA=2"),
        ("wet_without_henry", "PWETA_GAS=0.1"),
        ("density_without_diameter", "PDENSITY=1000"),
        ("width", "PDIA=0.00001,PDSIGMA=1"),
        ("shape", "PSHAPE=2"),
        ("version_zero", "VERSION=0"),
        ("version_future", "VERSION=2"),
        ("malformed", "NAME="),
    ] {
        let directory = tempfile::tempdir().expect("species directory");
        std::fs::write(directory.path().join("tracer.spec"), input).expect("species input");
        let parsed = SpeciesConfig::load_dir(directory.path());
        if let Ok(species) = &parsed {
            let item = &species[0];
            let kind = match item.species_kind() { SpeciesKind::Gas => "Gas", SpeciesKind::Aerosol => "Aerosol" };
            records.insert(format!("species/{label}/switches"), format!("{kind}:dry={},below={},in={},decay={},passive={}",item.dry_deposition_active(),item.below_cloud_wet_active(),item.in_cloud_wet_active(),item.decay_active(),item.is_passive_tracer()));
        }
        // Error contexts contain temporary directories; preserve the rest byte-for-byte.
        let mut species_records = BTreeMap::new();
        record(&mut species_records, &format!("species/{label}"), parsed);
        for (key, value) in species_records {
            records.insert(key, value.replace(
                    &directory.path().join("tracer.spec").display().to_string(),
                    "<SPECIES>/tracer.spec",
                ));
        }
    }
    let original = simulation();
    record(&mut records, "simulation/legacy", Ok(original.clone()));
    for (label, mutate) in [
        (
            "versioned",
            (|c: &mut SimulationConfig| c.version = Some(1)) as fn(&mut SimulationConfig),
        ),
        ("future", |c| c.version = Some(2)),
        ("empty_releases", |c| c.releases.clear()),
        ("too_many_species", |c| {
            c.species = vec![c.species[0].clone(); 5]
        }),
        ("mass_cardinality", |c| {
            c.releases[0].species_masses_kg = Some(vec![1.0, 2.0])
        }),
        ("validation_order", |c| {
            c.version = Some(2);
            c.command.start_time = "9999".into();
            c.releases.clear();
        }),
    ] {
        let mut config = original.clone();
        mutate(&mut config);
        record(
            &mut records,
            &format!("simulation/{label}"),
            config.validate().map(|()| config),
        );
    }
    let mut legacy = serde_json::to_value(&original).expect("legacy document");
    legacy
        .as_object_mut()
        .expect("simulation object")
        .remove("version");
    legacy["species"][0]
        .as_object_mut()
        .expect("species object")
        .remove("version");
    let decoded: SimulationConfig =
        serde_json::from_value(legacy).expect("omitted legacy versions");
    assert_eq!(decoded, original);
    let snapshot = serde_json::to_string_pretty(&records).expect("transcript") + "\n";
    assert_eq!(
        snapshot,
        include_str!("fixtures/config-behavior-v1.json").replace("\r\n", "\n")
    );
}

#[test]
fn test_config_file_loading_preserves_order_defaults_and_missing_paths() {
    let base = tempfile::tempdir().expect("configuration directory");
    let files = [
        ("COMMAND", COMMAND),
        ("RELEASES", RELEASE),
        ("OUTGRID", GRID),
    ];
    for (name, content) in files {
        let error = SimulationConfig::load(base.path()).expect_err("missing required file");
        assert!(
            matches!(error, ConfigError::MissingPath { path } if path == base.path().join(name))
        );
        std::fs::write(base.path().join(name), content).expect("configuration file");
    }
    assert!(
        matches!(SimulationConfig::load(base.path()), Err(ConfigError::MissingPath { path }) if path == base.path().join("SPECIES"))
    );
    let species_dir = base.path().join("SPECIES");
    std::fs::create_dir(&species_dir).expect("species directory");
    assert!(
        matches!(SimulationConfig::load(base.path()), Err(ConfigError::Validation { message }) if message == "SPECIES directory is empty")
    );
    std::fs::write(species_dir.join("b.spec"), "NAME=second").expect("second species");
    std::fs::write(species_dir.join("a.spec"), "NAME=first").expect("first species");
    std::fs::create_dir(species_dir.join("ignored")).expect("ignored directory");
    let config = SimulationConfig::load(base.path()).expect("complete configuration");
    assert_eq!(config.version, None);
    assert_eq!(
        config
            .species
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    assert_eq!(config.releases[0], simulation().releases[0]);
    assert_eq!(config.command, simulation().command);
    assert_eq!(config.outgrid, simulation().outgrid);
    assert!(
        matches!(SpeciesConfig::load_dir(&base.path().join("COMMAND")), Err(ConfigError::Parse { message, .. }) if message == "expected SPECIES to be a directory")
    );
}
