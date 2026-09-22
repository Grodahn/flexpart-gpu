//! Issue #71: validation of the frozen FLEXPART 11.1 interpolation oracle contract.
//!
//! `fixtures/interpolation/contract-v1.json` embeds inputs and the golden outputs of
//! the direct interpolation oracle driver (`scripts/interpolation/
//! direct_interpolation_oracle.f90`), which calls the real pinned routines
//! (find_grid_indices, find_grid_distances, find_z_level_meters, find_vert_vars,
//! hor_interpol_4d, temporal_interpolation, vert_interpol, interpol_rain).
//!
//! These tests (a) check the artifact/provenance metadata is frozen, and (b) verify
//! that every golden value satisfies the documented FLEXPART closed-form equations.
//! The independent closed-form re-check is the contract: any future implementation
//! that reproduces the goldens necessarily reproduces the pinned formulas.

use serde::Deserialize;

pub const FLEXPART_PINNED_COMMIT: &str = "c70586c2b7f5258850705325881c61f557ea9bd8";
pub const ORACLE_OUTPUT_VERSION: &str = "FLEXPART_INTERPOLATION_ROUTINE_ORACLE_V1";

const REL_TOL: f64 = 1.0e-4;
const ABS_TOL: f64 = 1.0e-6;

fn assert_close(actual: f64, expected: f64, what: &str) {
    let diff = (actual - expected).abs();
    let scale = expected.abs().max(1.0) * REL_TOL;
    assert!(
        diff <= scale + ABS_TOL,
        "{what}: golden {expected} != closed form {actual} (diff {diff})"
    );
}

fn as_f64(value: &serde_json::Value) -> f64 {
    value.as_f64().expect("golden value must be a number")
}

fn parse_f64(token: &str, what: &str) -> f64 {
    token
        .trim()
        .parse::<f64>()
        .unwrap_or_else(|_| panic!("invalid f64 in {what}: {token:?}"))
}

/// Split one driver input line into whitespace-separated tokens.
fn tokens(line: &str) -> Vec<String> {
    line.split_whitespace().map(str::to_string).collect()
}

/// x-fastest column look-up on the FLEXPART grid with the wrapped duplicate column.
fn field_ref(field: &[f64], nx: usize, ny: usize, ix: usize, jy: usize, nxmax: usize) -> f64 {
    debug_assert!(ix < nxmax && jy < ny);
    // Column canon_nx is the wrapped duplicate of column 0 (periodic grids).
    let x = if ix == nx { 0 } else { ix };
    field[x + nx * jy]
}

/// Freeze of `find_grid_indices` (interpol_mod.f90:128-166, mother-grid path).
fn flexpart_indices(xt: f64, yt: f64, nxmax: usize, nymax: usize) -> (usize, usize, usize, usize) {
    let ix = xt as usize;
    let jy = yt as usize;
    let ixp = ix + 1;
    let mut jyp = jy + 1;
    if jyp >= nymax {
        jyp -= 1;
    }
    let ixp = if ixp >= nxmax { ixp - nxmax } else { ixp };
    (ix, jy, ixp, jyp)
}

/// Freeze of `find_grid_distances` (interpol_mod.f90:168-188).
fn flexpart_weights(xt: f64, yt: f64, ix: usize, jy: usize) -> [f64; 4] {
    let ddx = xt - ix as f64;
    let ddy = yt - jy as f64;
    let rddx = 1.0 - ddx;
    let rddy = 1.0 - ddy;
    [rddx * rddy, ddx * rddy, rddx * ddy, ddx * ddy]
}

/// Closed-form horizontal sampling (hor_interpol_4d, interpol_mod.f90:481-492).
fn horizontal_value(field: &[f64], nx: usize, ny: usize, xt: f64, yt: f64, periodic: bool) -> f64 {
    let nxmax = if periodic { nx + 1 } else { nx };
    let (ix, jy, ixp, jyp) = flexpart_indices(xt, yt, nxmax, ny);
    let weights = flexpart_weights(xt, yt, ix, jy);
    weights[0] * field_ref(field, nx, ny, ix, jy, nxmax)
        + weights[1] * field_ref(field, nx, ny, ixp, jy, nxmax)
        + weights[2] * field_ref(field, nx, ny, ix, jyp, nxmax)
        + weights[3] * field_ref(field, nx, ny, ixp, jyp, nxmax)
}

/// Freeze of `find_z_level_meters` + `find_vert_vars_lin`
/// (interpol_mod.f90:215-242, 406-430). Returns 1-based (indz, indzp),
/// the lin-interpolation weights (dz1, dz2) and the bound flags.
fn vertical_levels(heights: &[f64], zt: f64) -> (usize, usize, f64, f64, [bool; 2]) {
    let nz = heights.len();
    if zt <= heights[0] {
        return (1, 2, 0.0, 1.0, [true, false]);
    }
    if zt >= heights[nz - 1] {
        return (nz - 1, nz, 1.0, 0.0, [false, true]);
    }
    for upper in 1..nz {
        if heights[upper] > zt {
            let dz = 1.0 / (heights[upper] - heights[upper - 1]);
            return (
                upper,
                upper + 1,
                (zt - heights[upper - 1]) * dz,
                (heights[upper] - zt) * dz,
                [false, false],
            );
        }
    }
    unreachable!("zt in (heights[0], heights[last]) must land in a layer")
}

/// Freeze of `temporal_interpolation` (interpol_mod.f90:531-537).
fn temporal_value(memtime: [f64; 2], itime: f64, time1: f64, time2: f64) -> f64 {
    let dt1 = itime - memtime[0];
    let dt2 = memtime[1] - itime;
    let dtt = 1.0 / (dt1 + dt2);
    (time1 * dt2 + time2 * dt1) * dtt
}

fn read_reals(input: &[String], cursor: &mut usize, count: usize, what: &str) -> Vec<f64> {
    let start = *cursor;
    *cursor += count;
    assert!(
        *cursor <= input.len(),
        "input truncated reading {count} values of {what}"
    );
    input[start..*cursor]
        .iter()
        .map(|line| parse_f64(line, what))
        .collect()
}

fn golden_queries(case: &FixtureCase, nquery: usize) -> &[serde_json::Value] {
    let queries = case.golden["queries"]
        .as_array()
        .expect("golden.queries must be an array");
    assert_eq!(queries.len(), nquery, "golden query count matches input");
    queries
}

fn run_horizontal_check(case: &FixtureCase) {
    let input = &case.input;
    let nx = tokens(&input[1])[0].parse::<usize>().unwrap();
    let ny = tokens(&input[1])[1].parse::<usize>().unwrap();
    let periodic = tokens(&input[3])[0].parse::<usize>().unwrap() != 0;
    let mut cursor = 4;
    let field = read_reals(input, &mut cursor, nx * ny, "horizontal field");
    let nquery = input[cursor].parse::<usize>().unwrap();
    cursor += 1;

    for (index, query) in golden_queries(case, nquery).iter().enumerate() {
        let query_tokens = tokens(&input[cursor + index]);
        let xt = parse_f64(&query_tokens[0], "xt");
        let yt = parse_f64(&query_tokens[1], "yt");
        let expected = horizontal_value(&field, nx, ny, xt, yt, periodic);
        assert_close(
            as_f64(&query["VALUE"][0]),
            expected,
            &format!("{} query {index}", case.id),
        );
    }
}

fn run_horizontal_geographic_check(case: &FixtureCase) {
    let input = &case.input;
    let nx = tokens(&input[1])[0].parse::<usize>().unwrap();
    let ny = tokens(&input[1])[1].parse::<usize>().unwrap();
    let grid = tokens(&input[2]);
    let xlon0 = parse_f64(&grid[0], "xlon0");
    let ylat0 = parse_f64(&grid[1], "ylat0");
    let dx = parse_f64(&grid[2], "dx");
    let dy = parse_f64(&grid[3], "dy");
    let periodic = tokens(&input[3])[0].parse::<usize>().unwrap() != 0;
    let mut cursor = 4;
    let field = read_reals(input, &mut cursor, nx * ny, "geographic horizontal field");
    let nquery = input[cursor].parse::<usize>().unwrap();
    cursor += 1;

    for (index, query) in golden_queries(case, nquery).iter().enumerate() {
        let query_tokens = tokens(&input[cursor + index]);
        let lon = parse_f64(&query_tokens[0], "longitude");
        let lat = parse_f64(&query_tokens[1], "latitude");
        let expected_xt = (lon - xlon0) / dx;
        let expected_yt = (lat - ylat0) / dy;

        assert_close(
            as_f64(&query["LONLAT"][0]),
            lon,
            &format!("{} longitude {index}", case.id),
        );
        assert_close(
            as_f64(&query["LONLAT"][1]),
            lat,
            &format!("{} latitude {index}", case.id),
        );
        assert_close(
            as_f64(&query["XY"][0]),
            expected_xt,
            &format!("{} coordtrafo xt {index}", case.id),
        );
        assert_close(
            as_f64(&query["XY"][1]),
            expected_yt,
            &format!("{} coordtrafo yt {index}", case.id),
        );
        let expected = horizontal_value(&field, nx, ny, expected_xt, expected_yt, periodic);
        assert_close(
            as_f64(&query["VALUE"][0]),
            expected,
            &format!("{} geographic query {index}", case.id),
        );
    }
}

fn run_vertical_check(case: &FixtureCase) {
    let input = &case.input;
    let mut cursor = 1;
    let nz = input[cursor].parse::<usize>().unwrap();
    cursor += 1;
    let heights = read_reals(input, &mut cursor, nz, "vertical heights");
    let nvalues = input[cursor].parse::<usize>().unwrap();
    cursor += 1;
    let values = read_reals(input, &mut cursor, nvalues, "vertical values");
    let nquery = input[cursor].parse::<usize>().unwrap();
    cursor += 1;
    assert_eq!(nvalues, nz, "one value per level");

    for (index, query) in golden_queries(case, nquery).iter().enumerate() {
        let query_tokens = tokens(&input[cursor + index]);
        let coordinate = query_tokens[0]
            .parse::<u32>()
            .expect("vertical coordinate id");
        let expected_coordinate = match case.vertical_staggering.as_deref() {
            Some("level_center") => 0,
            Some("level_interface") => 1,
            other => panic!(
                "vertical case {} must declare level_center/level_interface staggering, got {other:?}",
                case.id
            ),
        };
        assert_eq!(
            coordinate, expected_coordinate,
            "{} query {index} coordinate id must match declared staggering",
            case.id
        );
        let zt = parse_f64(&query_tokens[1], "zt");
        let (indz, indzp, dz1, dz2, _bounds) = vertical_levels(&heights, zt);
        let expected = values[indz - 1] * dz2 + values[indzp - 1] * dz1;
        assert_close(
            as_f64(&query["VALUE"][0]),
            expected,
            &format!("{} query {index}", case.id),
        );
        assert_eq!(query["LEVELS"][0].as_f64(), Some(indz as f64));
        assert_eq!(query["LEVELS"][1].as_f64(), Some(indzp as f64));
    }
}

fn run_temporal_check(case: &FixtureCase) {
    let input = &case.input;
    let mem_tokens = tokens(&input[1]);
    let memtime = [
        parse_f64(&mem_tokens[0], "memtime[0]"),
        parse_f64(&mem_tokens[1], "memtime[1]"),
    ];
    let nquery = input[2].parse::<usize>().unwrap();

    for (index, query) in golden_queries(case, nquery).iter().enumerate() {
        let query_tokens = tokens(&input[3 + index]);
        let itime = parse_f64(&query_tokens[0], "itime");
        let time1 = parse_f64(&query_tokens[1], "time1");
        let time2 = parse_f64(&query_tokens[2], "time2");
        let expected = temporal_value(memtime, itime, time1, time2);
        assert_close(
            as_f64(&query["VALUE"][0]),
            expected,
            &format!("{} query {index}", case.id),
        );
    }
}

fn run_rain_check(case: &FixtureCase) {
    let input = &case.input;
    let grid_tokens = tokens(&input[1]);
    let nx = grid_tokens[0].parse::<usize>().unwrap();
    let ny = grid_tokens[1].parse::<usize>().unwrap();
    let periodic = tokens(&input[3])[0].parse::<usize>().unwrap() != 0;
    let mem_tokens = tokens(&input[4]);
    let memtime = [
        parse_f64(&mem_tokens[0], "memtime[0]"),
        parse_f64(&mem_tokens[1], "memtime[1]"),
    ];

    let mut cursor = 5;
    let lsp_t1 = read_reals(input, &mut cursor, nx * ny, "lsp t1");
    let lsp_t2 = read_reals(input, &mut cursor, nx * ny, "lsp t2");
    let cp_t1 = read_reals(input, &mut cursor, nx * ny, "cp t1");
    let cp_t2 = read_reals(input, &mut cursor, nx * ny, "cp t2");
    let tcc_t1 = read_reals(input, &mut cursor, nx * ny, "tcc t1");
    let tcc_t2 = read_reals(input, &mut cursor, nx * ny, "tcc t2");
    let tt_t1 = read_reals(input, &mut cursor, nx * ny, "tt t1");
    let tt_t2 = read_reals(input, &mut cursor, nx * ny, "tt t2");
    let ctwc_t1 = read_reals(input, &mut cursor, nx * ny, "ctwc t1");
    let ctwc_t2 = read_reals(input, &mut cursor, nx * ny, "ctwc t2");
    let nquery = input[cursor].parse::<usize>().unwrap();
    cursor += 1;

    for (index, query) in golden_queries(case, nquery).iter().enumerate() {
        let query_tokens = tokens(&input[cursor + index]);
        let xt = parse_f64(&query_tokens[0], "xt");
        let yt = parse_f64(&query_tokens[1], "yt");
        let itime = parse_f64(&query_tokens[2], "itime");

        let bilinear = |field: &[f64]| horizontal_value(field, nx, ny, xt, yt, periodic);

        let dt1 = itime - memtime[0];
        let dt2 = memtime[1] - itime;
        let dt = memtime[1] - memtime[0];
        // Frozen quirk (interpol_mod.f90:1309): dtt = dt/3 even for numpf=1.
        let dtt = dt / 3.0;

        let lsp_expected = (bilinear(&lsp_t1) * dt2 + bilinear(&lsp_t2) * dt1) / dtt;
        let cp_expected = (bilinear(&cp_t1) * dt2 + bilinear(&cp_t2) * dt1) / dtt;
        let tcc_expected = (bilinear(&tcc_t1) * dt2 + bilinear(&tcc_t2) * dt1) / dt;
        let tt_expected = (bilinear(&tt_t1) * dt2 + bilinear(&tt_t2) * dt1) / dt;
        let ctwc_expected = (bilinear(&ctwc_t1) * dt2 + bilinear(&ctwc_t2) * dt1) / dt;

        assert_close(
            as_f64(&query["LSP"][0]),
            lsp_expected,
            &format!("{} LSP", case.id),
        );
        assert_close(
            as_f64(&query["CP"][0]),
            cp_expected,
            &format!("{} CP", case.id),
        );
        assert_close(
            as_f64(&query["TCC"][0]),
            tcc_expected,
            &format!("{} TCC", case.id),
        );
        assert_close(
            as_f64(&query["TT"][0]),
            tt_expected,
            &format!("{} TT", case.id),
        );
        assert_close(
            as_f64(&query["CTWC"][0]),
            ctwc_expected,
            &format!("{} CTWC", case.id),
        );

        // All four interpolation corners carry icmv (-9999), so the masked cloud
        // average collapses to icmv (interpol_mod.f90:1400-1414, 1568-1573).
        assert_eq!(query["CLOUD"][0].as_f64(), Some(-9999.0));
        assert_eq!(query["CLOUD"][1].as_f64(), Some(-9999.0));
    }
}

#[test]
fn contract_fixture_metadata_is_frozen() {
    let source = include_str!("../fixtures/interpolation/contract-v1.json");
    let contract: ContractFixture =
        serde_json::from_str(source).expect("parse interpolation contract");

    assert_eq!(contract.schema.id, "flexpart-gpu.interpolation-contract");
    assert_eq!(contract.schema.version, 1);
    assert_eq!(contract.oracle_output_version, ORACLE_OUTPUT_VERSION);
    assert_eq!(
        contract.pinned_flexpart.pinned_commit, FLEXPART_PINNED_COMMIT,
        "contract must pin the same FLEXPART revision as reference/flexpart-11.1.json"
    );
    assert_eq!(
        contract.pinned_flexpart.version, "11.1",
        "contract must reference FLEXPART 11.1"
    );
    assert_eq!(
        contract.real_data_samples.len(),
        1,
        "contract must represent one real #29/#30 ERA5/ETEX sample"
    );
    let real = &contract.real_data_samples[0];
    assert_eq!(real["id"], "era5-etex-real-column-v1");
    assert_eq!(real["kind"], "era5_etex_vertical_column");
    assert_eq!(real["selection"]["native_levels"], 137);
    assert_eq!(
        real["source"]["canonical_fixture"],
        "fixtures/meteorology/era5-etex-native-v1.json"
    );
    assert_eq!(
        real["compatibility"]["canonical_contract_issue"], 29,
        "real sample must be anchored to the #29 canonical contract"
    );
    assert_eq!(
        real["compatibility"]["vertical_transform_issue"], 30,
        "real sample must use the #30 vertical-transform path"
    );
    assert_eq!(real["compatibility"]["interpolation_contract_issue"], 71);
    assert_eq!(
        real["compatibility"]["interpolation_sampling_case"],
        "real-era5-etex-temperature-column"
    );

    let real_sampling = contract
        .cases
        .iter()
        .find(|case| case.id == "real-era5-etex-temperature-column")
        .expect("real ERA5/ETEX sampling fixture");
    assert_eq!(
        real_sampling.vertical_staggering.as_deref(),
        Some("level_center")
    );
    assert_eq!(real_sampling.golden["NLEVEL"].as_f64(), Some(137.0));
    assert_eq!(real_sampling.golden["NQUERY"].as_f64(), Some(5.0));
    assert_eq!(real_sampling.semantics["units"]["value"], "kelvin");
    assert_eq!(
        real_sampling.semantics["time"]["timestamp"],
        "1994-10-23T15:00:00Z"
    );
    let real_source = real_sampling
        .source_oracle
        .as_ref()
        .expect("real sampling fixture must identify #30 direct-oracle geometry");
    assert_eq!(real_source["producer_issue"], 30);
    assert_eq!(
        real_source["oracle_output_sha256"],
        "7fe3f5fa17d0067464258c93efbe1bbf9cf2403b4d442191b08b6deb29e026a4"
    );
    assert_eq!(
        real_source["value_source"],
        "fixtures/meteorology/era5-etex-native-v1.json"
    );
    assert_eq!(
        real["compatibility"]["interpolation_geometry_oracle_sha256"],
        real_source["oracle_output_sha256"]
    );

    let geographic = contract
        .cases
        .iter()
        .find(|case| case.id == "horizontal-geographic-interior")
        .expect("geographic horizontal fixture");
    assert_eq!(geographic.mode, "horizontal_geographic");
    assert_eq!(
        geographic.semantics["production_call_path"][0],
        "point_mod::coordtrafo"
    );
    assert_eq!(geographic.semantics["mapping"]["dx_deg"], 0.25);

    let model = contract
        .cases
        .iter()
        .find(|case| case.id == "vertical-model-levels")
        .expect("model-level vertical fixture");
    assert_eq!(model.vertical_staggering.as_deref(), Some("level_center"));

    let interface = contract
        .cases
        .iter()
        .find(|case| case.id == "vertical-interface-wzlev")
        .expect("W/interface vertical fixture");
    assert_eq!(
        interface.vertical_staggering.as_deref(),
        Some("level_interface")
    );
    let source = interface
        .source_oracle
        .as_ref()
        .expect("W/interface fixture must identify its direct FLEXPART source");
    assert_eq!(source["producer_issue"], 30);
    assert_eq!(
        source["producer_routine"],
        "verttransform_mod::verttransform_ecmwf_heights"
    );
    assert_eq!(source["geometry_field"], "wzlev");
    assert_eq!(source["value_field"], "omega * pinmconv");
    assert_eq!(
        source["oracle_output_sha256"],
        "5015ea3a9a9e42b1a2b88c60c2867b74a632bffd1b9cfefdc186b005c752b197"
    );

    for case in &contract.cases {
        for key in ["coordinates", "staggering", "ordering", "units", "time"] {
            assert!(
                case.semantics[key].is_object(),
                "{} must declare semantics.{key}",
                case.id
            );
        }
    }
    let temporal = contract
        .cases
        .iter()
        .find(|case| case.id == "temporal-bilinear")
        .expect("temporal fixture");
    assert_eq!(
        temporal.semantics["time"]["primitive_outside_memory_window"],
        "linear_extrapolation_no_range_guard"
    );
    assert_eq!(
        temporal.semantics["time"]["production_call_path"],
        serde_json::json!([
            "getfields_mod::getfields",
            "advance_mod::advance",
            "interpol_mod::init_interpol",
            "interpol_mod::find_time_vars",
            "interpol_mod::temporal_interpolation"
        ])
    );
    assert_eq!(
        temporal.semantics["time"]["range_policy_owner"],
        "caller/canonical API; Petterssen end-step guard is outside find_time_vars/temporal_interpolation"
    );

    let rain = contract
        .cases
        .iter()
        .find(|case| case.id == "rain-layer-fields")
        .expect("rain fixture");
    assert_eq!(
        rain.semantics["time"]["precipitation_input_representation"],
        "already_normalized_rate"
    );
    assert_eq!(
        rain.semantics["time"]["reset_deaccumulation"],
        "not_performed_here; owned_by_issue_75"
    );
    assert_eq!(
        real["semantics"]["ordering"]["vertical"],
        "increasing",
        "real sample must freeze canonical vertical ordering"
    );
    assert_eq!(
        real["compatibility"]["extraction_source_sha256"],
        "bd1e9d8531ceabe99544a7e7659766d1642fcb576a0fdd505aa4c50886f3bdc0"
    );
}

#[test]
fn contract_provenance_matches_fixture() {
    let provenance_source = include_str!("../fixtures/interpolation/contract-v1.provenance.json");
    let provenance: ContractProvenance = serde_json::from_str(provenance_source)
        .expect("interpolation provenance must be well-formed");
    assert_eq!(
        provenance.schema,
        "flexpart-gpu.interpolation-contract-provenance.v1"
    );
    assert_eq!(provenance.pinned_commit, FLEXPART_PINNED_COMMIT);
    assert!(provenance.checkout_clean, "oracle checkout must be clean");
    assert!(provenance.binary_entrypoint_present);
    assert_eq!(
        provenance.real_data_samples.len(),
        1,
        "provenance must carry the real #29/#30 ERA5/ETEX sample"
    );
    assert_eq!(
        provenance.real_data_samples[0]["id"],
        "era5-etex-real-column-v1"
    );
    assert_eq!(
        provenance.interface_vertical_source["oracle_output_sha256"],
        "5015ea3a9a9e42b1a2b88c60c2867b74a632bffd1b9cfefdc186b005c752b197"
    );
    assert_eq!(
        provenance.interface_vertical_source["geometry_field"],
        "wzlev"
    );
    assert_eq!(
        provenance.real_vertical_source["oracle_output_sha256"],
        "7fe3f5fa17d0067464258c93efbe1bbf9cf2403b4d442191b08b6deb29e026a4"
    );
    assert_eq!(
        provenance
            .cases
            .get("real-era5-etex-temperature-column")
            .map(String::as_str),
        Some("5679760f10c75679ecd597979b5caadfd3bf30debbbf9b0fa1fc6af1aa9e3771")
    );
    assert_eq!(
        provenance.fixture_artifact["path"],
        "fixtures/interpolation/contract-v1.json"
    );
    assert_eq!(
        provenance.fixture_artifact["hash_kind"],
        "normalized_canonical_json_sha256"
    );
    assert_eq!(
        provenance.fixture_artifact["sha256"],
        "6729843cbd59b0ac60e39e873fde179dbe7949f3191ee0ee7ba9deb443d45c70"
    );
    assert_eq!(
        provenance.generator_source["path"],
        "scripts/interpolation/prepare_interpolation_fixtures.py"
    );
    assert_eq!(
        provenance.oracle_harness_source["path"],
        "scripts/interpolation/direct_oracle.sh"
    );
    assert_eq!(
        provenance.real_extraction_source["path"],
        "scripts/vertical/extract_real_etex_column.py"
    );
    assert_eq!(
        provenance.reference_manifest["path"],
        "reference/flexpart-11.1.json"
    );

    // The linked objects must name every module the oracle driver calls.
    let source_files: Vec<&str> = provenance
        .linked_flexpart
        .objects
        .iter()
        .map(|object| object.file.as_str())
        .collect();
    for expected in [
        "src/com_mod.f90",
        "src/par_mod.f90",
        "src/windfields_mod.f90",
        "src/interpol_mod.f90",
    ] {
        assert!(
            source_files.contains(&expected),
            "provenance must record {expected}"
        );
    }
    let obligations: Vec<&str> = vec![
        "coordtrafo",
        "find_grid_indices",
        "find_grid_distances",
        "find_time_vars",
        "find_z_level_meters",
        "find_vert_vars",
        "hor_interpol_4d",
        "hor_interpol_2d",
        "temporal_interpolation",
        "vert_interpol",
        "interpol_rain",
    ];
    for routine in obligations {
        assert!(
            provenance
                .linked_flexpart
                .routines
                .contains(&routine.to_string()),
            "provenance must name the frozen routine {routine}"
        );
    }
    let golden_files: Vec<&str> = provenance.cases.keys().map(String::as_str).collect();
    for name in [
        "horizontal-interior",
        "horizontal-geographic-interior",
        "horizontal-periodic-wrap",
        "vertical-model-levels",
        "vertical-interface-wzlev",
        "temporal-bilinear",
        "rain-layer-fields",
        "real-era5-etex-temperature-column",
    ] {
        assert!(
            golden_files.contains(&name),
            "provenance must record golden case {name}"
        );
    }
}

#[test]
fn goldens_satisfy_flexpart_closed_forms() {
    let source = include_str!("../fixtures/interpolation/contract-v1.json");
    let contract: ContractFixture =
        serde_json::from_str(source).expect("parse interpolation contract");

    let mut seen = std::collections::HashSet::new();
    for case in &contract.cases {
        assert!(
            seen.insert(case.id.as_str()),
            "duplicate case id {}",
            case.id
        );
        match case.mode.as_str() {
            "horizontal" => run_horizontal_check(case),
            "horizontal_geographic" => run_horizontal_geographic_check(case),
            "vertical" => run_vertical_check(case),
            "temporal" => run_temporal_check(case),
            "rain" => run_rain_check(case),
            other => panic!("unknown oracle case mode {other}"),
        }
    }
    // Every frozen sampling mode must be covered by at least one case.
    for mode in ["horizontal", "horizontal_geographic", "vertical", "temporal", "rain"] {
        assert!(
            contract.cases.iter().any(|case| case.mode == mode),
            "contract must contain at least one {mode} case"
        );
    }
}

#[derive(Deserialize)]
struct ContractFixture {
    schema: ContractSchema,
    pinned_flexpart: PinnedFlexpart,
    oracle_output_version: String,
    real_data_samples: Vec<serde_json::Value>,
    cases: Vec<FixtureCase>,
}

#[derive(Deserialize)]
struct ContractSchema {
    id: String,
    version: u32,
}

#[derive(Deserialize)]
struct PinnedFlexpart {
    version: String,
    pinned_commit: String,
}

#[derive(Deserialize)]
struct FixtureCase {
    id: String,
    mode: String,
    #[serde(default)]
    vertical_staggering: Option<String>,
    #[serde(default)]
    source_oracle: Option<serde_json::Value>,
    semantics: serde_json::Value,
    input: Vec<String>,
    golden: serde_json::Map<String, serde_json::Value>,
}

#[derive(Deserialize)]
struct ContractProvenance {
    schema: String,
    pinned_commit: String,
    checkout_clean: bool,
    #[serde(rename = "entrypoint_present")]
    binary_entrypoint_present: bool,
    linked_flexpart: LinkedFlexpart,
    cases: std::collections::HashMap<String, String>,
    real_data_samples: Vec<serde_json::Value>,
    interface_vertical_source: serde_json::Value,
    real_vertical_source: serde_json::Value,
    fixture_artifact: serde_json::Value,
    generator_source: serde_json::Value,
    oracle_harness_source: serde_json::Value,
    real_extraction_source: serde_json::Value,
    reference_manifest: serde_json::Value,
}

#[derive(Deserialize)]
struct LinkedFlexpart {
    objects: Vec<LinkedObject>,
    routines: Vec<String>,
}

#[derive(Deserialize)]
struct LinkedObject {
    file: String,
}
