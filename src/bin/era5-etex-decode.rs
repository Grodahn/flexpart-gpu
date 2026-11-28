//! Offline CLI for bounded native ERA5/ETEX diagnostics and verified snapshots.

use anyhow::{bail, Context, Result};
use flexpart_gpu::io::era5_etex::{decode_model_levels, load_snapshot, load_snapshot_with_surface};
use std::io;
use std::path::Path;

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [mode, path, epoch, ancillary @ ..] = args.as_slice() else {
        bail!(
            "usage: era5-etex-decode <diagnostic|snapshot> <model-level.grib> <UTC-epoch-seconds> [lnsp.grib]"
        );
    };
    let epoch: i64 = epoch
        .to_str()
        .context("epoch must be a UTC epoch in ASCII seconds")?
        .parse()
        .context("epoch must be an integer")?;
    let stdout = io::stdout().lock();
    match (mode.to_str(), ancillary) {
        (Some("diagnostic"), []) => {
            serde_json::to_writer(stdout, &decode_model_levels(Path::new(path), epoch)?)?;
        }
        (Some("snapshot"), []) => {
            serde_json::to_writer(stdout, &load_snapshot(Path::new(path), epoch)?)?;
        }
        (Some("snapshot"), [surface]) => {
            let result = load_snapshot_with_surface(Path::new(path), Path::new(surface), epoch)?;
            serde_json::to_writer(stdout, &result)?;
        }
        _ => bail!("select diagnostic or snapshot mode with the appropriate sources"),
    }
    Ok(())
}
