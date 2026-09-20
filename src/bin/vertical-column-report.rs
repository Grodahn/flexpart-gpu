use std::{env, fs, path::PathBuf};

use anyhow::{bail, Context, Result};
use flexpart_gpu::meteorology::{vertical::reconstruct_vertical_geometry, Snapshot};
use serde::Serialize;

#[derive(Debug, Serialize)]
struct CandidateColumnReport {
    schema_version: u32,
    source_snapshot: String,
    result: flexpart_gpu::meteorology::vertical::VerticalTransformResult,
}

fn main() -> Result<()> {
    let mut args = env::args_os().skip(1);
    let input = args
        .next()
        .map(PathBuf::from)
        .context("usage: vertical-column-report <snapshot.json> <output.json>")?;
    let output = args
        .next()
        .map(PathBuf::from)
        .context("usage: vertical-column-report <snapshot.json> <output.json>")?;
    if args.next().is_some() {
        bail!("usage: vertical-column-report <snapshot.json> <output.json>");
    }

    let source = fs::read_to_string(&input)
        .with_context(|| format!("read canonical snapshot {}", input.display()))?;
    let snapshot: Snapshot = serde_json::from_str(&source)
        .with_context(|| format!("parse canonical snapshot {}", input.display()))?;
    let result = reconstruct_vertical_geometry(&snapshot)
        .with_context(|| format!("reconstruct vertical geometry {}", input.display()))?;

    let report = CandidateColumnReport {
        schema_version: 1,
        source_snapshot: input.to_string_lossy().into_owned(),
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
