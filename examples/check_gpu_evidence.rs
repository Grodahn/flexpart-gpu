//! Validate retained paired execution evidence through the repository GPU contract.

use anyhow::{Context, Result};
use flexpart_gpu::gpu::GpuCalculationEvidence;

fn main() -> Result<()> {
    let path = std::env::args()
        .nth(1)
        .context("expected a comparison report path")?;
    let report: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).context("read comparison report")?)?;
    let evidence: GpuCalculationEvidence = serde_json::from_value(
        report
            .get("gpu_evidence")
            .context("missing paired GPU evidence")?
            .clone(),
    )?;
    evidence
        .require_paired_pass()
        .context("paired GPU evidence failed the normative contract")?;
    println!("Paired GPU evidence passed: {}", evidence.case_id);
    Ok(())
}
