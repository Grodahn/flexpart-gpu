//! #114 deposition meteorology consumer inventory lock.
//!
//! The inventory records that no production deposition kernel samples
//! meteorology. These checks fail when a kernel binding, a deposition forcing
//! field or a canonical field identity changes without an inventory update, or
//! when a row loses its status or blocker link.

use flexpart_gpu::meteorology::FIELD_SPECS;
use flexpart_gpu::simulation::ForwardStepForcing;
use serde_json::Value;
use std::collections::BTreeSet;

const INVENTORY: &str =
    include_str!("../fixtures/deposition/meteorology-consumer-inventory-v1.json");
const STATUSES: [&str; 3] = ["MIGRATED", "NOT_APPLICABLE", "BLOCKED"];
const ROW_KEYS: [&str; 14] = [
    "id",
    "consumer",
    "flexpart_routine",
    "field",
    "units",
    "production_source",
    "execution",
    "canonical_field",
    "canonical_temporal_semantics",
    "existing_api",
    "grid_and_vertical",
    "missing",
    "feasibility",
    "status",
];

fn inventory() -> Value {
    serde_json::from_str(INVENTORY).expect("inventory JSON")
}

/// `(binding, variable name)` pairs; each `@binding` line is followed by its `var`.
fn wgsl_bindings(source: &str) -> Vec<(u64, String)> {
    let mut bindings = Vec::new();
    let mut pending = None;
    for line in source.lines().map(str::trim) {
        if let Some(rest) = line.strip_prefix("@group(0) @binding(") {
            let index = rest.split_once(')').expect("binding index").0;
            pending = Some(index.parse().expect("numeric binding"));
        } else if let Some(index) = pending.take() {
            let declaration = line.split_once('>').expect("var qualifier").1;
            let name = declaration.split(':').next().expect("var name").trim();
            bindings.push((index, name.to_owned()));
        }
    }
    bindings
}

#[test]
fn test_deposition_inventory_rows_have_status_and_linked_blockers() {
    let inventory = inventory();
    assert_eq!(inventory["issue"], 114);
    assert_eq!(
        inventory["flexpart_revision"],
        "c70586c2b7f5258850705325881c61f557ea9bd8"
    );
    let rows = inventory["rows"].as_array().expect("rows");
    let mut ids = BTreeSet::new();
    for row in rows {
        for key in ROW_KEYS {
            assert!(row.get(key).is_some(), "{} lacks {key}", row["id"]);
        }
        assert!(ids.insert(row["id"].as_str().unwrap()), "duplicate row id");
        let status = row["status"].as_str().unwrap();
        assert!(STATUSES.contains(&status), "{status}");
        let blockers = row["blockers"].as_array().expect("blockers");
        for blocker in blockers {
            let text = blocker.as_str().unwrap();
            assert!(
                text.starts_with('#') && text[1..].parse::<u32>().is_ok(),
                "blocker must be an issue link: {text}"
            );
        }
        if status == "BLOCKED" {
            assert!(
                !blockers.is_empty(),
                "{} is blocked without a link",
                row["id"]
            );
        }
        // No deposition meteorology input is consumed by production today.
        assert_ne!(
            status, "MIGRATED",
            "{} claims an unproven migration",
            row["id"]
        );
    }
    for required in [
        "DRY-VDEP",
        "DRY-USTAR",
        "DRY-T2M",
        "DRY-PS",
        "DRY-RH",
        "DRY-OLI",
        "DRY-SSR",
        "DRY-PRECIP",
        "DRY-SNOW",
        "DRY-LANDUSE",
        "DRY-SEASON",
        "WET-COEFFICIENTS",
        "WET-LSP",
        "WET-CP",
        "WET-TCC",
        "WET-CTWC",
        "WET-TEMPERATURE",
        "WET-CLOUD-BOUNDS",
        "WET-NUMPF3",
    ] {
        assert!(ids.contains(required), "missing inventory row {required}");
    }
    let precipitation = rows
        .iter()
        .filter(|row| ["WET-LSP", "WET-CP"].contains(&row["id"].as_str().unwrap()));
    for row in precipitation {
        let blockers: Vec<_> = row["blockers"].as_array().unwrap().iter().collect();
        assert!(
            blockers.iter().any(|b| *b == "#227"),
            "particle-scale #119 blocker"
        );
    }
}

#[test]
fn test_deposition_inventory_canonical_fields_exist() {
    let known: BTreeSet<String> = FIELD_SPECS
        .iter()
        .map(|spec| format!("{:?}", spec.id))
        .collect();
    for row in inventory()["rows"].as_array().unwrap() {
        let Some(fields) = row["canonical_field"].as_str() else {
            continue;
        };
        for name in fields.split(['+', '/']).map(str::trim) {
            assert!(
                known.contains(name),
                "{}: unknown canonical field {name}",
                row["id"]
            );
        }
    }
}

#[test]
fn test_deposition_inventory_matches_kernel_bindings() {
    let inventory = inventory();
    for kernel in inventory["production_kernels"].as_array().unwrap() {
        let path = kernel["shader"].as_str().unwrap();
        let source = std::fs::read_to_string(path).expect("deposition shader");
        let declared = wgsl_bindings(&source);
        let recorded: Vec<(u64, String)> = kernel["bindings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| {
                (
                    b["binding"].as_u64().unwrap(),
                    b["name"].as_str().unwrap().to_owned(),
                )
            })
            .collect();
        assert_eq!(
            declared, recorded,
            "{path} bindings changed; update the #114 inventory"
        );
        for binding in kernel["bindings"].as_array().unwrap() {
            assert_ne!(
                binding["class"], "meteorology",
                "a meteorology binding needs a MIGRATED row with production evidence"
            );
        }
    }
}

#[test]
fn test_deposition_forcing_surface_is_inventoried() {
    // Exhaustive destructuring: a new forcing field fails to compile until it is inventoried.
    let ForwardStepForcing {
        settling_enabled: _,
        dry_deposition_velocity_m_s: _,
        wet_scavenging_coefficient_s_inv: _,
        wet_precipitating_fraction: _,
        decay_constant_s_inv: _,
        rho_grad_over_rho: _,
    } = ForwardStepForcing::default();
    let sources: Vec<String> = inventory()["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["production_source"].as_str().unwrap().to_owned())
        .collect();
    for field in [
        "dry_deposition_velocity_m_s",
        "wet_scavenging_coefficient_s_inv",
        "wet_precipitating_fraction",
    ] {
        assert!(
            sources.iter().any(|s| s.contains(field)),
            "{field} not inventoried"
        );
    }
}
