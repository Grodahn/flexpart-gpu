//! Shared canonical inputs for the direct #118 U/V device fixtures.
use flexpart_gpu::meteorology::Snapshot;
use serde_json::{json, Value};

pub fn oracle() -> Value {
    let path = std::env::var("FLEXPART_GPU_SHARED_HEIGHT_ORACLE")
        .unwrap_or_else(|_| "fixtures/interpolation/shared-height-v1/report.json".into());
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

pub fn snapshot(memory: usize) -> Snapshot {
    let report = oracle();
    let mut base: Value = serde_json::from_str(include_str!(
        "../../fixtures/vertical/synthetic-column-v1.json"
    ))
    .unwrap();
    base["horizontal_grid"]["nx"] = json!(2);
    base["horizontal_grid"]["ny"] = json!(2);
    let coord = &mut base["vertical_coordinate"];
    coord["hybrid_a_interface_pa"] = json!([10000.0, 10000.0, 5000.0, 0.0]);
    coord["hybrid_b_interface"] = json!([0.1, 0.3, 0.7, 1.0]);
    coord["interface_values"] = json!([20000.0, 40000.0, 75000.0, 100000.0]);
    coord["level_values"] = json!([30000.0, 57500.0, 87500.0]);
    let columns: Vec<_> = report["decoded_inputs"]["columns_repeated_in_y"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["memory"] == memory)
        .collect();
    let original = base["fields"].as_array().unwrap().clone();
    let specs = [
        ("surface_pressure", "pascal", "non_negative", None),
        ("temperature", "kelvin", "signed_scalar", Some(0)),
        (
            "specific_humidity",
            "kilogram_per_kilogram",
            "non_negative",
            Some(1),
        ),
        ("wind_u", "meter_per_second", "positive_eastward", Some(2)),
        ("wind_v", "meter_per_second", "positive_northward", Some(3)),
        ("orography", "meter", "signed_scalar", None),
        ("temperature2m", "kelvin", "signed_scalar", None),
        ("dewpoint2m", "kelvin", "signed_scalar", None),
        ("wind_u10m", "meter_per_second", "positive_eastward", None),
        ("wind_v10m", "meter_per_second", "positive_northward", None),
    ];
    let mut fields = Vec::new();
    for (id, unit, sign, lane) in specs {
        let mut f = original[usize::from(lane.is_some())].clone();
        f["id"] = json!(id);
        f["unit"] = json!(unit);
        f["sign"] = json!(sign);
        f["shape"] = if lane.is_some() {
            json!([2, 2, 3])
        } else {
            json!([2, 2])
        };
        let mut values = Vec::new();
        for z in 0..if lane.is_some() { 3 } else { 1 } {
            for _y in 0..2 {
                for (x, c) in columns.iter().enumerate() {
                    let value = match lane {
                        Some(l) => c["rows_t_k_q_kg_kg_u_m_s_v_m_s_omega_pa_s"][3 - z][l].clone(),
                        None => match id {
                            "surface_pressure" => c["pressure_pa"].clone(),
                            "temperature2m" => c["temperature_2m_k"].clone(),
                            "dewpoint2m" => c["dewpoint_2m_k"].clone(),
                            "orography" => report["decoded_inputs"]["terrain_m_asl"][x].clone(),
                            _ => c["rows_t_k_q_kg_kg_u_m_s_v_m_s_omega_pa_s"][0]
                                [if id == "wind_u10m" { 2 } else { 3 }]
                            .clone(),
                        },
                    };
                    values.push(value);
                }
            }
        }
        f["values"] = json!(values);
        f["time"]["valid_time_epoch_seconds"] = json!((memory - 1) * 3600);
        if id == "orography" {
            f["time"]["kind"] = json!("static");
        }
        fields.push(f);
    }
    base["fields"] = json!(fields);
    serde_json::from_value(base).unwrap()
}

pub fn revision() -> String {
    std::env::var("FLEXPART_GPU_CANDIDATE_REVISION").unwrap_or_else(|_| {
        String::from_utf8(
            std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .output()
                .unwrap()
                .stdout,
        )
        .unwrap()
        .trim()
        .into()
    })
}
