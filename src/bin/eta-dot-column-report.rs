use std::{env, fs, path::PathBuf};

use anyhow::{bail, Context, Result};
use flexpart_gpu::meteorology::{
    vertical::{eta_dot_to_pressure_velocity, EtaDotPressureVelocity, NativeVerticalMotion},
    Snapshot,
};
use serde::Serialize;

#[derive(Debug, Serialize)]
struct SchemaIdentityReport {
    id: &'static str,
    version: u32,
}

#[derive(Debug, Serialize)]
struct EtaDotColumnReport {
    schema: SchemaIdentityReport,
    source_snapshot: String,
    source_motion: String,
    result: EtaDotPressureVelocity,
}

fn main() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let snapshot_path = args.next().map(PathBuf::from).context(
        "usage: eta-dot-column-report <snapshot.json> <eta-dot-motion.json> <output.json>",
    )?;
    let motion_path = args.next().map(PathBuf::from).context(
        "usage: eta-dot-column-report <snapshot.json> <eta-dot-motion.json> <output.json>",
    )?;
    let output = args.next().map(PathBuf::from).context(
        "usage: eta-dot-column-report <snapshot.json> <eta-dot-motion.json> <output.json>",
    )?;
    if args.next().is_some() {
        bail!("usage: eta-dot-column-report <snapshot.json> <eta-dot-motion.json> <output.json>");
    }

    let source = fs::read_to_string(&snapshot_path)
        .with_context(|| format!("read canonical snapshot {}", snapshot_path.display()))?;
    let snapshot: Snapshot = serde_json::from_str(&source)
        .with_context(|| format!("parse canonical snapshot {}", snapshot_path.display()))?;
    let motion_source = fs::read_to_string(&motion_path)
        .with_context(|| format!("read native eta-dot motion {}", motion_path.display()))?;
    let motion: NativeVerticalMotion = serde_json::from_str(&motion_source)
        .with_context(|| format!("parse native eta-dot motion {}", motion_path.display()))?;

    let result = eta_dot_to_pressure_velocity(&snapshot, &motion).with_context(|| {
        format!(
            "eta-dot preprocessing {} + {}",
            snapshot_path.display(),
            motion_path.display()
        )
    })?;

    let report = EtaDotColumnReport {
        schema: SchemaIdentityReport {
            id: "flexpart-gpu.eta-dot-column-report",
            version: 1,
        },
        source_snapshot: snapshot_path.to_string_lossy().into_owned(),
        source_motion: motion_path.to_string_lossy().into_owned(),
        result,
    };
    let encoded = serde_json::to_string_pretty(&report)?;
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create output directory {}", parent.display()))?;
    }
    fs::write(&output, format!("{encoded}\n"))
        .with_context(|| format!("write candidate report {}", output.display()))?;
    Ok(())
}
