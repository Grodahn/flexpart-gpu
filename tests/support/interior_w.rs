//! Exact #186 fixture inputs; no independent scientific calculations.
use flexpart_gpu::meteorology::{vertical::*, *};
use serde_json::{json, Value};

pub fn oracle() -> Value {
    let path = std::env::var("FLEXPART_GPU_INTERIOR_W_ORACLE")
        .unwrap_or_else(|_| "fixtures/interpolation/interior-w-v1/report.json".into());
    serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap()
}

pub fn snapshot(memory: usize) -> Snapshot {
    let report = oracle();
    let mut base: Value = serde_json::from_str(include_str!(
        "../../fixtures/vertical/synthetic-column-v1.json"
    ))
    .unwrap();
    base["horizontal_grid"]["nx"] = json!(3);
    base["horizontal_grid"]["ny"] = json!(3);
    let coord = &mut base["vertical_coordinate"];
    coord["hybrid_a_interface_pa"] = json!([10000.0, 10000.0, 5000.0, 0.0]);
    coord["hybrid_b_interface"] = json!([0.1, 0.3, 0.7, 1.0]);
    coord["interface_values"] = json!([20000.0, 40000.0, 75000.0, 100000.0]);
    coord["level_values"] = json!([30000.0, 57500.0, 87500.0]);
    base["horizontal_grid"]["xlon0_deg"] = json!(-2.0);
    base["horizontal_grid"]["ylat0_deg"] = json!(48.0);
    base["horizontal_grid"]["dx_deg"] = json!(0.25);
    base["horizontal_grid"]["dy_deg"] = json!(0.4);
    let columns: Vec<_> = report["scientific"]["decoded_inputs"]["columns"]
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
            json!([3, 3, 3])
        } else {
            json!([3, 3])
        };
        let mut values = Vec::new();
        for z in 0..if lane.is_some() { 3 } else { 1 } {
            for y in 0..3 {
                for x in 0..3 {
                    let c = columns[y * 3 + x];
                    let value = match lane {
                        Some(l) => c["center_t_q_u_v_interface_omega"][3 - z][l].clone(),
                        None => match id {
                            "surface_pressure" => c["surface_pa_t_k_td_k"][0].clone(),
                            "temperature2m" => c["surface_pa_t_k_td_k"][1].clone(),
                            "dewpoint2m" => c["surface_pa_t_k_td_k"][2].clone(),
                            "orography" => report["scientific"]["decoded_inputs"]
                                ["terrain_m_asl_y_x"][y][x]
                                .clone(),
                            _ => c["center_t_q_u_v_interface_omega"][0]
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

pub fn native(memory: usize) -> NativeVerticalMotion {
    let report = oracle();
    let columns: Vec<_> = report["scientific"]["decoded_inputs"]["columns"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["memory"] == memory)
        .collect();
    let mut values = Vec::new();
    for z in (0..4).rev() {
        for c in &columns {
            values.push(c["center_t_q_u_v_interface_omega"][z][4].as_f64().unwrap() as f32);
        }
    }
    NativeVerticalMotion {
        kind: NativeVerticalMotionKind::PressureVelocityOmega,
        unit: NativeVerticalMotionUnit::PascalPerSecond,
        sign: NativeVerticalMotionSign::PositivePressureIncreasing,
        vertical_staggering: VerticalStaggering::LevelInterface,
        values,
        provenance: NativeVerticalMotionProvenance {
            source_id: format!("interior-w-186-time-{memory}"),
        },
    }
}
