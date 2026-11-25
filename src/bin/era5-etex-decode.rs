//! Offline CLI for the bounded native ERA5/ETEX diagnostic.

use anyhow::{bail, Context, Result};
use flexpart_gpu::io::era5_etex::{decode_model_levels, load_snapshot};
use std::io;
use std::path::Path;

fn main() -> Result<()> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let [mode, path, epoch] = args.as_slice() else {
        bail!(
            "usage: era5-etex-decode <diagnostic|snapshot> <model-level.grib> <UTC-epoch-seconds>"
        );
    };
    let epoch: i64 = epoch
        .to_str()
        .context("epoch must be a UTC epoch in ASCII seconds")?
        .parse()
        .context("epoch must be an integer")?;
    let stdout = io::stdout().lock();
    match mode.to_str() {
        Some("diagnostic") => {
            serde_json::to_writer(stdout, &decode_model_levels(Path::new(path), epoch)?)?;
        }
        Some("snapshot") => {
            serde_json::to_writer(stdout, &load_snapshot(Path::new(path), epoch)?)?;
        }
        _ => bail!("select diagnostic or snapshot mode"),
    }
    Ok(())
}
