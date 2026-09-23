use std::{env, fs, path::PathBuf};

use anyhow::{bail, Context, Result};
use flexpart_gpu::meteorology::{
    temporal::{build_comparison_report, OracleQuery, Tolerance, CANDIDATE_DESCRIPTION},
    FieldId, SchemaIdentity, Snapshot,
};
use serde::Deserialize;

pub const SCENARIO_SCHEMA_ID: &str = "flexpart-gpu.temporal-interpolation-scenario";

#[derive(Debug, Deserialize)]
struct Scenario {
    schema: SchemaIdentity,
    scenario_id: String,
    field_id: FieldId,
    snapshot_paths: Vec<String>,
    tolerance: Tolerance,
    queries: Vec<OracleQuery>,
}

fn main() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let input = args
        .next()
        .map(PathBuf::from)
        .context("usage: temporal-interpolation-report <scenario.json> <output.json>")?;
    let output = args
        .next()
        .map(PathBuf::from)
        .context("usage: temporal-interpolation-report <scenario.json> <output.json>")?;
    if args.next().is_some() {
        bail!("usage: temporal-interpolation-report <scenario.json> <output.json>");
    }

    let scenario_dir = input
        .parent()
        .map_or_else(|| PathBuf::from("."), PathBuf::from);
    let source =
        fs::read_to_string(&input).with_context(|| format!("read scenario {}", input.display()))?;
    let scenario: Scenario = serde_json::from_str(&source)
        .with_context(|| format!("parse scenario {}", input.display()))?;
    if scenario.schema.id != SCENARIO_SCHEMA_ID {
        bail!(
            "unexpected scenario schema {} (expected {SCENARIO_SCHEMA_ID})",
            scenario.schema.id
        );
    }

    let mut snapshots: Vec<Snapshot> = Vec::with_capacity(scenario.snapshot_paths.len());
    for path in &scenario.snapshot_paths {
        let resolved = scenario_dir.join(path);
        let bytes = fs::read_to_string(&resolved)
            .with_context(|| format!("read snapshot {}", resolved.display()))?;
        let snapshot: Snapshot = serde_json::from_str(&bytes)
            .with_context(|| format!("parse snapshot {}", resolved.display()))?;
        snapshots.push(snapshot);
    }
    let snapshot_refs: Vec<&Snapshot> = snapshots.iter().collect();

    let report = build_comparison_report(
        &scenario.scenario_id,
        scenario.field_id,
        &snapshot_refs,
        scenario.tolerance,
        &scenario.queries,
    )
    .with_context(|| {
        format!(
            "build temporal comparison report for {}",
            scenario.scenario_id
        )
    })?;

    let encoded = serde_json::to_string_pretty(&report).expect("serialize comparison report");
    if let Some(parent) = output.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .with_context(|| format!("create output dir {}", parent.display()))?;
        }
    }
    fs::write(&output, encoded)
        .with_context(|| format!("write comparison report {}", output.display()))?;

    if !report.status.is_pass() {
        bail!(
            "candidate {} failed scenario {}; report written to {}",
            CANDIDATE_DESCRIPTION,
            report.scenario_id,
            output.display()
        );
    }
    Ok(())
}
