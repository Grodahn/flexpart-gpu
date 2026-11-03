//! Regression lock for the #231 ICON-EU vertical-geometry decision.
//!
//! The decision records, per question, whether DWD ICON-EU height-based
//! terrain-following geometry (HHL) can produce the canonical #29/#30 vertical
//! runtime geometry. This test downloads and decodes nothing. It fails closed
//! when the committed decision, its decoded evidence, the sample request or the
//! retrieval manifest drift apart, when a verdict or normative reference is
//! lost, and it re-derives the decisive per-column checks offline from the
//! retained complete columns. The FLEXPART height replay used for the
//! reconstruction comparison is tied to the pinned compiled routine output
//! frozen by #118. Reproduction: docs/research/icon-eu-vertical-231.md.

use serde_json::Value;
use sha2::{Digest, Sha256};

const DECISION_PATH: &str = "fixtures/meteorology/icon-eu-vertical-231-v1/decision.json";
const EVIDENCE_PATH: &str = "fixtures/meteorology/icon-eu-vertical-231-v1/decoded-evidence.json";
const REQUEST_PATH: &str = "fixtures/meteorology/icon-eu-vertical-231-v1/sample-request.json";
const MANIFEST_PATH: &str = "fixtures/meteorology/icon-eu-vertical-231-v1/retrieval-manifest.json";
const FLEXPART_REFERENCE_PATH: &str = "reference/flexpart-11.1.json";
const SHARED_HEIGHT_INPUT_PATH: &str = "fixtures/interpolation/shared-height-v1/input.txt";
const SHARED_HEIGHT_OUTPUT_PATH: &str = "fixtures/interpolation/shared-height-v1/output.txt";
/// Pinned SHA-256 of the canonical LF decision artifact; update deliberately.
const DECISION_SHA256: &str = "cd987e2ccbdd953f0f826c6973ba2f4bd28abb8cf247f05f461c11e1f6f383b3";
const HALF_LEVELS: usize = 75;
const FULL_LEVELS: usize = 74;
const SAMPLE_FILE_COUNT: usize = 301;
const VERDICTS: [&str; 3] = [
    "SUPPORTED_WITH_PROVEN_TRANSFORMATION",
    "REQUIRES_SCHEMA_EXTENSION",
    "UNSUPPORTED",
];
const EXPECTED_QUESTIONS: [(&str, &str); 5] = [
    (
        "model_level_height_authority",
        "SUPPORTED_WITH_PROVEN_TRANSFORMATION",
    ),
    (
        "full_level_height_convention",
        "SUPPORTED_WITH_PROVEN_TRANSFORMATION",
    ),
    (
        "interface_height_coordinate",
        "SUPPORTED_WITH_PROVEN_TRANSFORMATION",
    ),
    ("interface_pressure", "UNSUPPORTED"),
    ("canonical_schema", "REQUIRES_SCHEMA_EXTENSION"),
];
/// ICON release-2024.10-public `mo_physical_constants.f90:96,108,121`.
const ICON_RD: f64 = 287.04;
const ICON_GRAV: f64 = 9.80665;
const ICON_RV: f64 = 461.51;
/// FLEXPART 11.1 `par_mod.f90:61` and `verttransform_mod.f90:1998-2018`.
const FLEXPART_R_AIR: f64 = 287.05;
const FLEXPART_GA: f64 = 9.81;
const FLEXPART_SURFACE_VAPOUR_FACTOR: f64 = 0.378;
const FLEXPART_MOIST_FACTOR: f64 = 0.608;
const FLEXPART_TV_SWITCH_K: f64 = 0.2;
/// #30 predeclared height tolerance (docs/vertical-transform.md): the replay
/// must reproduce the pinned single-precision routine within it.
const HEIGHT_ABS_TOL_M: f64 = 0.02;
const HEIGHT_REL_TOL: f64 = 1.0e-5;
/// Offline recomputation of double-precision derived evidence; allows libm and
/// `serde_json` float parsing differences, not a scientific tolerance.
const RECOMPUTE_ABS_TOL: f64 = 1.0e-6;
const RECOMPUTE_REL_TOL: f64 = 1.0e-10;

fn read_canonical(path: &str) -> String {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("{path}: {error}"))
        .replace("\r\n", "\n")
}

fn sha256_hex(text: &str) -> String {
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

fn parse(path: &str) -> Value {
    serde_json::from_str(&read_canonical(path)).unwrap_or_else(|error| panic!("{path}: {error}"))
}

fn text<'a>(value: &'a Value, key: &str) -> &'a str {
    value[key]
        .as_str()
        .unwrap_or_else(|| panic!("missing string `{key}` in {value}"))
}

fn number(value: &Value, key: &str) -> f64 {
    value[key]
        .as_f64()
        .unwrap_or_else(|| panic!("missing number `{key}`"))
}

fn numbers(value: &Value, key: &str) -> Vec<f64> {
    value[key]
        .as_array()
        .unwrap_or_else(|| panic!("missing array `{key}`"))
        .iter()
        .map(|item| item.as_f64().expect("number"))
        .collect()
}

fn recomputed(actual: f64, recorded: f64) -> bool {
    (actual - recorded).abs() <= RECOMPUTE_ABS_TOL + RECOMPUTE_REL_TOL * recorded.abs()
}

/// FLEXPART `qvsat_mod.f90:144-196` saturation vapour pressure over water [Pa].
/// The literal 0.43429 is FLEXPART's truncated log10(e) and must stay as written.
#[allow(clippy::approx_constant)]
fn flexpart_ew(temperature_k: f64) -> f64 {
    let y = 373.16 / temperature_k;
    let a = -7.90298 * (y - 1.0) + 5.02808 * 0.43429 * y.ln();
    let c = -1.3816 * (-1.0 + 10f64.powf((1.0 - 1.0 / y) * 11.344)) / 1.0e7;
    let d = 8.1328 * (-1.0 + 10f64.powf((1.0 - y) * 3.49149)) / 1.0e3;
    101_324.6 * 10f64.powf(a + c + d)
}

/// Surface boundary and top-down model-level state of one column.
struct Column {
    surface_pressure_pa: f64,
    temperature_2m_k: f64,
    dewpoint_2m_k: f64,
    pressure_pa: Vec<f64>,
    temperature_k: Vec<f64>,
    humidity_kg_kg: Vec<f64>,
}

/// Double-precision replay of FLEXPART's hypsometric AGL model-level heights,
/// `verttransform_mod.f90:1998-2023`, integrated from the surface upward with
/// the column's level pressures in place of `akz + bkz * ps`. Top-down result.
fn flexpart_heights(column: &Column) -> Vec<f64> {
    let scale = FLEXPART_R_AIR / FLEXPART_GA;
    let mut tv_old = column.temperature_2m_k
        * (1.0
            + FLEXPART_SURFACE_VAPOUR_FACTOR * flexpart_ew(column.dewpoint_2m_k)
                / column.surface_pressure_pa);
    let mut p_old = column.surface_pressure_pa;
    let mut height = 0.0;
    let mut heights = vec![0.0; column.pressure_pa.len()];
    for level in (0..column.pressure_pa.len()).rev() {
        let tv = column.temperature_k[level]
            * (1.0 + FLEXPART_MOIST_FACTOR * column.humidity_kg_kg[level]);
        let log_pressure = (p_old / column.pressure_pa[level]).ln();
        let layer_tv = if (tv - tv_old).abs() > FLEXPART_TV_SWITCH_K {
            (tv - tv_old) / (tv / tv_old).ln()
        } else {
            tv
        };
        height += scale * log_pressure * layer_tv;
        heights[level] = height;
        tv_old = tv;
        p_old = column.pressure_pa[level];
    }
    heights
}

/// FLEXPART `wzlev` construction (`verttransform_mod.f90:2025-2029`) from
/// bottom-up model-level heights that start with the artificial 0 m level.
fn flexpart_wzlev(bottom_up_levels: &[f64]) -> Vec<f64> {
    let count = bottom_up_levels.len();
    let mut wzlev = vec![0.0; count];
    for k in 1..count - 1 {
        wzlev[k] = bottom_up_levels[k + 1].midpoint(bottom_up_levels[k]);
    }
    wzlev[count - 1] = wzlev[count - 2] + bottom_up_levels[count - 1] - bottom_up_levels[count - 2];
    wzlev
}

fn decision() -> Value {
    parse(DECISION_PATH)
}

fn evidence() -> Value {
    parse(EVIDENCE_PATH)
}

fn columns(evidence: &Value) -> &Vec<Value> {
    evidence["columns"].as_array().expect("columns")
}

fn evidence_column(value: &Value) -> Column {
    Column {
        surface_pressure_pa: number(value, "ps_pa"),
        temperature_2m_k: number(value, "t2m_k"),
        dewpoint_2m_k: number(value, "td2m_k"),
        pressure_pa: numbers(value, "p_pa"),
        temperature_k: numbers(value, "t_k"),
        humidity_kg_kg: numbers(value, "qv_kg_kg"),
    }
}

fn question<'a>(decision: &'a Value, id: &str) -> &'a Value {
    decision["questions"]
        .as_array()
        .expect("questions")
        .iter()
        .find(|item| item["id"] == id)
        .unwrap_or_else(|| panic!("missing question {id}"))
}

#[test]
fn test_icon_eu_vertical_decision_digest_is_pinned() {
    assert_eq!(
        sha256_hex(&read_canonical(DECISION_PATH)),
        DECISION_SHA256,
        "decision artifact changed; review and update DECISION_SHA256 deliberately"
    );
}

#[test]
fn test_icon_eu_vertical_decision_binds_evidence_inputs() {
    let decision = decision();
    let evidence = evidence();
    let bound = &decision["evidence"];
    for (key, path) in [
        ("decoded_evidence", EVIDENCE_PATH),
        ("sample_request", REQUEST_PATH),
        ("retrieval_manifest", MANIFEST_PATH),
    ] {
        assert_eq!(text(&bound[key], "path"), path);
        assert_eq!(
            text(&bound[key], "sha256_lf"),
            sha256_hex(&read_canonical(path)),
            "{path} drifted from the decision"
        );
    }
    assert_eq!(
        text(&evidence, "request_sha256_lf"),
        sha256_hex(&read_canonical(REQUEST_PATH))
    );
    assert_eq!(
        text(&evidence, "retrieval_manifest_sha256_lf"),
        sha256_hex(&read_canonical(MANIFEST_PATH))
    );
    assert_eq!(evidence["issue"], 231);
    assert_eq!(
        text(&evidence, "status"),
        "DERIVED_EVIDENCE_RAW_BYTES_AUTHORITATIVE"
    );
}

#[test]
fn test_icon_eu_vertical_decision_has_one_verdict_per_question() {
    let decision = decision();
    let questions = decision["questions"].as_array().expect("questions");
    assert_eq!(questions.len(), EXPECTED_QUESTIONS.len());
    for (id, expected) in EXPECTED_QUESTIONS {
        let entry = question(&decision, id);
        let verdict = text(entry, "verdict");
        assert!(
            VERDICTS.contains(&verdict),
            "{id}: unknown verdict {verdict}"
        );
        assert_eq!(verdict, expected, "{id}: verdict changed");
        assert!(
            !entry["normative_reference"]
                .as_array()
                .expect("normative_reference")
                .is_empty(),
            "{id}: normative reference missing"
        );
        if verdict != "SUPPORTED_WITH_PROVEN_TRANSFORMATION" {
            assert!(
                !entry["follow_up_issues"]
                    .as_array()
                    .expect("follow_up_issues")
                    .is_empty(),
                "{id}: a non-supported verdict needs a follow-up issue"
            );
        }
    }
    let authority = question(&decision, "model_level_height_authority");
    assert_eq!(text(authority, "authority"), "provider_hhl");
    assert_eq!(
        text(&authority["rejected_alternative"], "verdict"),
        "UNSUPPORTED"
    );
}

#[test]
fn test_icon_eu_vertical_references_match_pins() {
    let decision = decision();
    let flexpart = parse(FLEXPART_REFERENCE_PATH);
    assert_eq!(
        text(
            &decision["normative_references"]["flexpart"],
            "pinned_commit"
        ),
        text(&flexpart, "pinned_commit")
    );
    let icon = &decision["normative_references"]["icon"];
    assert_eq!(text(icon, "release_tag"), "release-2024.10-public");
    for source in icon["sources"].as_array().expect("icon sources") {
        assert_eq!(text(source, "sha256").len(), 64, "ICON source hash missing");
    }
}

#[test]
fn test_icon_eu_vertical_evidence_identity_is_unique_and_independently_decoded() {
    let evidence = evidence();
    let messages = evidence["messages"].as_array().expect("messages");
    assert_eq!(messages.len(), SAMPLE_FILE_COUNT);
    assert_eq!(evidence["retrieval_file_count"], SAMPLE_FILE_COUNT);
    for message in messages {
        assert_eq!(
            message["independent_values_max_abs_difference"].as_f64(),
            Some(0.0),
            "{}: octet reader + libaec disagree with ecCodes",
            text(message, "path")
        );
    }
    let identity = &evidence["science"]["identity"];
    for key in [
        "distinct_grids",
        "distinct_data_date_time",
        "distinct_vertical_grid",
        "hhl_surfaces",
        "packing_templates",
    ] {
        assert_eq!(identity[key].as_array().expect(key).len(), 1, "{key}");
    }
    assert!(identity["surface_identity_problems"]
        .as_array()
        .expect("problems")
        .is_empty());
    let vertical_grid: Value = serde_json::from_str(
        identity["distinct_vertical_grid"][0]
            .as_str()
            .expect("vertical grid"),
    )
    .expect("vertical grid json");
    // ecCodes reports `nlev` as a double; it counts the 75 half levels.
    assert_eq!(vertical_grid[0].as_f64(), Some(75.0));
    assert_eq!(
        vertical_grid[2].as_str(),
        text(&decision()["provider"]["vertical_grid"], "uuid_of_vgrid").into()
    );
}

#[test]
/// The surface interface must be exactly 0 m AGL; no tolerance applies.
#[allow(clippy::float_cmp)]
fn test_icon_eu_vertical_full_field_checks_support_verdicts() {
    let decision = decision();
    let science = &evidence()["science"];
    assert_eq!(science["terrain"]["hhl75_equals_hsurf_everywhere"], true);
    assert_eq!(
        science["half_levels"]["strictly_decreasing_with_level_index"],
        true
    );
    assert_eq!(
        science["full_level_pressure"]["strictly_increasing_with_level_index"],
        true
    );
    assert_eq!(
        science["full_level_pressure"]["lowest_below_surface_pressure_everywhere"],
        true
    );
    let closest = &science["full_level_convention"]["fraction_closest_to_hydrostatic"];
    let mean = number(closest, "half_level_arithmetic_mean");
    for alternative in ["upper_half_level", "lower_half_level"] {
        assert!(mean > number(closest, alternative), "{alternative}");
    }
    let claims = &decision["evidence_claims"];
    let reconstruction = &science["flexpart_reconstruction_minus_hhl_full_levels"];
    assert!(recomputed(
        number(&reconstruction["all_levels"], "max_abs"),
        number(claims, "flexpart_reconstruction_max_abs_m")
    ));
    assert!(recomputed(
        number(
            &science["interfaces"]["wzlev_style_from_hhl_full_minus_hhl_interior_m"],
            "max_abs"
        ),
        number(claims, "wzlev_style_interior_max_abs_m")
    ));
    assert!(recomputed(
        number(
            &science["interface_pressure_candidates"]["arithmetic_minus_loglinear_pa"],
            "max_abs"
        ),
        number(claims, "interface_pressure_candidate_spread_max_abs_pa")
    ));
    assert_eq!(
        number(&science["interfaces"]["surface_interface_agl_m"], "max_abs"),
        0.0
    );
}

#[test]
/// Equal virtual temperatures select the exact FLEXPART-style log-mean limit.
#[allow(clippy::float_cmp)]
fn test_icon_eu_vertical_retained_columns_recompute_offline() {
    let evidence = evidence();
    let retained = columns(&evidence);
    assert!(retained.len() >= 7, "representative columns missing");
    for value in retained {
        let rule = text(value, "rule");
        let hhl = numbers(value, "hhl_m");
        let hsurf = number(value, "hsurf_m");
        let column = evidence_column(value);
        assert_eq!(hhl.len(), HALF_LEVELS, "{rule}");
        assert_eq!(column.pressure_pa.len(), FULL_LEVELS, "{rule}");
        assert!(
            hhl.windows(2).all(|pair| pair[0] > pair[1]),
            "{rule}: HHL order"
        );
        assert!(
            column.pressure_pa.windows(2).all(|pair| pair[0] < pair[1]),
            "{rule}: P order"
        );
        assert_eq!(
            hhl[HALF_LEVELS - 1].to_bits(),
            hsurf.to_bits(),
            "{rule}: HHL(75) != HSURF"
        );
        let derived = &value["derived"];
        let full_agl: Vec<f64> = (0..FULL_LEVELS)
            .map(|k| hhl[k].midpoint(hhl[k + 1]) - hsurf)
            .collect();
        for (actual, recorded) in full_agl.iter().zip(numbers(derived, "full_level_agl_m")) {
            assert!(recomputed(*actual, recorded), "{rule}: full-level AGL");
        }
        let replay = flexpart_heights(&column);
        for (actual, recorded) in replay.iter().zip(numbers(derived, "flexpart_replay_agl_m")) {
            assert!(recomputed(*actual, recorded), "{rule}: replay height");
        }
        for (k, recorded) in numbers(derived, "flexpart_replay_minus_hhl_full_m")
            .into_iter()
            .enumerate()
        {
            assert!(
                recomputed(replay[k] - full_agl[k], recorded),
                "{rule}: replay minus HHL"
            );
        }
        let wz_recorded = numbers(derived, "wzlev_style_from_hhl_full_minus_hhl_interior_m");
        assert_eq!(wz_recorded.len(), HALF_LEVELS - 2);
        for (h, recorded) in wz_recorded.into_iter().enumerate() {
            let interface = h + 1;
            let wz = full_agl[interface - 1].midpoint(full_agl[interface]);
            assert!(
                recomputed(wz - (hhl[interface] - hsurf), recorded),
                "{rule}: wzlev"
            );
            let second_difference =
                0.25 * (hhl[interface - 1] - 2.0 * hhl[interface] + hhl[interface + 1]);
            assert!(
                recomputed(second_difference, recorded),
                "{rule}: wzlev identity"
            );
        }
        let virtual_t: Vec<f64> = column
            .temperature_k
            .iter()
            .zip(&column.humidity_kg_kg)
            .map(|(t, q)| t * (1.0 + (ICON_RV / ICON_RD - 1.0) * q))
            .collect();
        for (k, recorded) in numbers(derived, "hypsometric_ratio_mean_convention")
            .into_iter()
            .enumerate()
        {
            let (upper, lower) = (virtual_t[k], virtual_t[k + 1]);
            let log_mean = if upper == lower {
                upper
            } else {
                (upper - lower) / (upper / lower).ln()
            };
            let thickness = ICON_RD / ICON_GRAV
                * log_mean
                * (column.pressure_pa[k + 1] / column.pressure_pa[k]).ln();
            let geometric = 0.5 * (hhl[k] - hhl[k + 2]);
            assert!(recomputed(thickness / geometric, recorded), "{rule}: ratio");
        }
        assert_column_boundaries(rule, derived, &hhl, &full_agl, &column);
    }
}

/// Re-derives the retained top-interface and surface-pressure closure values
/// of one column (`hhl` ASL top-down, `full_agl` top-down).
fn assert_column_boundaries(
    rule: &str,
    derived: &Value,
    hhl: &[f64],
    full_agl: &[f64],
    column: &Column,
) {
    let hsurf = hhl[HALF_LEVELS - 1];
    let mut bottom_up_full = vec![0.0];
    bottom_up_full.extend(full_agl.iter().rev());
    let top_wzlev = flexpart_wzlev(&bottom_up_full)[HALF_LEVELS - 1];
    assert!(
        recomputed(
            top_wzlev - (hhl[0] - hsurf),
            number(derived, "wzlev_style_top_minus_hhl_top_m")
        ),
        "{rule}: wzlev top"
    );
    let lowest = FULL_LEVELS - 1;
    let fraction = -full_agl[lowest] / (full_agl[lowest - 1] - full_agl[lowest]);
    let log_lowest = column.pressure_pa[lowest].ln();
    let log_next = column.pressure_pa[lowest - 1].ln();
    let surface = (log_lowest + (log_next - log_lowest) * fraction).exp();
    assert!(
        recomputed(
            surface - column.surface_pressure_pa,
            number(derived, "loglinear_surface_pressure_minus_ps_pa")
        ),
        "{rule}: surface pressure closure"
    );
}

#[test]
fn test_icon_eu_vertical_flexpart_replay_matches_pinned_routine_output() {
    let input: Vec<Vec<f64>> = read_canonical(SHARED_HEIGHT_INPUT_PATH)
        .lines()
        .map(|line| {
            line.split_whitespace()
                .map(|token| token.parse().expect("input number"))
                .collect()
        })
        .collect();
    let native_layers = 3;
    let half_levels = &input[1..=native_layers + 1];
    let mut oracle = std::collections::BTreeMap::new();
    for line in read_canonical(SHARED_HEIGHT_OUTPUT_PATH).lines().skip(1) {
        let tokens: Vec<&str> = line.split_whitespace().collect();
        if tokens[0] != "NATIVE" {
            continue;
        }
        let key: Vec<usize> = tokens[1..4]
            .iter()
            .map(|t| t.parse().expect("key"))
            .collect();
        let heights: Vec<f64> = tokens[4..6]
            .iter()
            .map(|t| t.parse().expect("height"))
            .collect();
        oracle.insert((key[0], key[1], key[2]), heights);
    }
    let mut cursor = native_layers + 3;
    let mut compared = 0;
    for memory in 1..=2 {
        for x in 0..=1 {
            let surface = &input[cursor];
            let rows = &input[cursor + 1..cursor + 2 + native_layers];
            cursor += native_layers + 2;
            let ps = surface[0];
            let half_pressure: Vec<f64> = half_levels.iter().map(|ab| ab[0] + ab[1] * ps).collect();
            // Real layers bottom-up are rows 1..; the canonical replay is top-down.
            let column = Column {
                surface_pressure_pa: ps,
                temperature_2m_k: surface[1],
                dewpoint_2m_k: surface[2],
                pressure_pa: (0..native_layers)
                    .rev()
                    .map(|k| half_pressure[k].midpoint(half_pressure[k + 1]))
                    .collect(),
                temperature_k: rows[1..].iter().rev().map(|row| row[0]).collect(),
                humidity_kg_kg: rows[1..].iter().rev().map(|row| row[1]).collect(),
            };
            let mut bottom_up = vec![0.0];
            bottom_up.extend(flexpart_heights(&column).iter().rev());
            let wzlev = flexpart_wzlev(&bottom_up);
            for level in 0..=native_layers {
                let expected = &oracle[&(memory, x, level + 1)];
                for (actual, pinned) in
                    [(bottom_up[level], expected[0]), (wzlev[level], expected[1])]
                {
                    assert!(
                        (actual - pinned).abs()
                            <= HEIGHT_ABS_TOL_M.max(HEIGHT_REL_TOL * pinned.abs()),
                        "memory {memory} x {x} level {level}: replay {actual} vs pinned {pinned}"
                    );
                    compared += 1;
                }
            }
        }
    }
    assert_eq!(compared, 32);
}
