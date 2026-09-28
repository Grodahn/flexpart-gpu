use std::path::PathBuf;

use anyhow::{anyhow, Context, Result};
use flexpart_gpu::gpu::{run_preflight_record, GpuExecutionStatus, GpuPreflightOptions};

const USAGE: &str = "\
GPU runtime preflight check.

Usage:
  cargo run --bin gpu-preflight -- [--backend <value>] [--software] [--no-smoke] [--json-output <path>]
  cargo run --bin gpu-preflight -- --help

Options:
  --backend <value>  Override backend selector (auto|vulkan|metal|dx12|gl|webgpu)
  --software         Request the software fallback adapter (same as FLEXPART_GPU_SOFTWARE=1).
                     Runs the real WGSL compute path on Lavapipe/WARP; timings must
                     not be used as GPU performance values.
  --force-fallback   Alias for --software
  --no-smoke         Skip tiny compute dispatch/readback smoke test
  --json-output <path>
                     Write the versioned machine-readable pass/fail record
  -h, --help         Show this help

Environment:
  FLEXPART_GPU_SOFTWARE=1 or WGPU_FORCE_FALLBACK_ADAPTER=1 selects the software adapter.
";

#[derive(Debug, Clone, PartialEq, Eq)]
struct CliOptions {
    backend_override: Option<String>,
    run_smoke_test: bool,
    force_software_fallback: bool,
    json_output: Option<PathBuf>,
}

impl Default for CliOptions {
    fn default() -> Self {
        Self {
            backend_override: None,
            run_smoke_test: true,
            force_software_fallback: false,
            json_output: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum CliCommand {
    Help,
    Run(CliOptions),
}

fn parse_cli_args<I>(args: I) -> Result<CliCommand>
where
    I: IntoIterator<Item = String>,
{
    let mut options = CliOptions::default();
    let mut iter = args.into_iter();
    while let Some(argument) = iter.next() {
        match argument.as_str() {
            "-h" | "--help" => return Ok(CliCommand::Help),
            "--no-smoke" => options.run_smoke_test = false,
            "--smoke" => options.run_smoke_test = true,
            "--software" | "--force-fallback" | "--fallback" => {
                options.force_software_fallback = true;
            }
            "--no-software" | "--no-fallback" => {
                options.force_software_fallback = false;
            }
            "--backend" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow!("missing value after --backend"))?;
                options.backend_override = Some(value);
            }
            "--json-output" => {
                let value = iter
                    .next()
                    .ok_or_else(|| anyhow!("missing value after --json-output"))?;
                options.json_output = Some(PathBuf::from(value));
            }
            _ if argument.starts_with("--backend=") => {
                let (_, value) = argument
                    .split_once('=')
                    .ok_or_else(|| anyhow!("invalid --backend argument"))?;
                options.backend_override = Some(value.to_string());
            }
            _ if argument.starts_with("--json-output=") => {
                let (_, value) = argument
                    .split_once('=')
                    .ok_or_else(|| anyhow!("invalid --json-output argument"))?;
                if value.is_empty() {
                    return Err(anyhow!("missing value after --json-output"));
                }
                options.json_output = Some(PathBuf::from(value));
            }
            _ => return Err(anyhow!("unknown argument: {argument}")),
        }
    }

    Ok(CliCommand::Run(options))
}

fn print_report(report: &flexpart_gpu::gpu::GpuPreflightReport) {
    println!("requested backend: {}", report.requested_backend);
    println!(
        "adapter: {} ({}, {})",
        report.adapter.name, report.adapter.backend, report.adapter.device_type
    );
    println!(
        "software fallback requested: {}",
        report.adapter.software_fallback_requested
    );
    let is_software_adapter =
        report.adapter.adapter_class == flexpart_gpu::gpu::GpuAdapterClass::SoftwareWgsl;
    println!("software adapter: {is_software_adapter}");
    if is_software_adapter {
        println!("note: software WGSL adapter in use; timings must not be used as GPU performance values");
    }
    println!(
        "device ids: vendor=0x{:04x} device=0x{:04x}",
        report.adapter.vendor_id, report.adapter.device_id
    );
    println!(
        "driver: {} | {}",
        report.adapter.driver, report.adapter.driver_info
    );
    println!("limits:");
    println!("  max_bind_groups: {}", report.limits.max_bind_groups);
    println!(
        "  max_storage_buffers_per_shader_stage: {}",
        report.limits.max_storage_buffers_per_shader_stage
    );
    println!(
        "  max_compute_invocations_per_workgroup: {}",
        report.limits.max_compute_invocations_per_workgroup
    );
    println!(
        "  max_compute_workgroup_size: ({}, {}, {})",
        report.limits.max_compute_workgroup_size_x,
        report.limits.max_compute_workgroup_size_y,
        report.limits.max_compute_workgroup_size_z
    );
    println!(
        "  max_compute_workgroups_per_dimension: {}",
        report.limits.max_compute_workgroups_per_dimension
    );
    println!("  max_buffer_size: {}", report.limits.max_buffer_size);
    println!(
        "  supports_wind_texture_sampling: {}",
        report.supports_wind_texture_sampling
    );
    match report.smoke_test.status {
        GpuExecutionStatus::Passed => {
            let value = report
                .smoke_test
                .actual_value
                .expect("passed smoke evidence always contains a value");
            println!("smoke test: PASS ({value})");
        }
        GpuExecutionStatus::Skipped => println!("smoke test: SKIPPED"),
        GpuExecutionStatus::Failed => println!("smoke test: FAILED"),
    }
}

fn run() -> Result<()> {
    env_logger::init();
    match parse_cli_args(std::env::args().skip(1).collect::<Vec<_>>())? {
        CliCommand::Help => {
            println!("{USAGE}");
            return Ok(());
        }
        CliCommand::Run(cli) => {
            let record = pollster::block_on(run_preflight_record(GpuPreflightOptions {
                backend_override: cli.backend_override,
                run_smoke_test: cli.run_smoke_test,
                force_software_fallback: cli.force_software_fallback,
            }));
            record
                .validate()
                .context("GPU preflight produced an invalid evidence record")?;
            if let Some(path) = cli.json_output {
                let json = serde_json::to_string_pretty(&record)
                    .context("failed to serialize GPU preflight record")?;
                std::fs::write(&path, format!("{json}\n")).with_context(|| {
                    format!("failed to write GPU preflight record to {}", path.display())
                })?;
            }
            match record.status {
                GpuExecutionStatus::Passed => {
                    println!("GPU preflight: OK");
                    let report = record
                        .report
                        .as_ref()
                        .ok_or_else(|| anyhow!("passing preflight record lacks report"))?;
                    print_report(report);
                }
                GpuExecutionStatus::Failed => {
                    return Err(anyhow!(
                        "{}",
                        record
                            .failure
                            .as_deref()
                            .unwrap_or("unknown preflight failure")
                    ));
                }
                GpuExecutionStatus::Skipped => {
                    println!("GPU preflight: SKIPPED (initialization only)");
                    let report = record
                        .report
                        .as_ref()
                        .ok_or_else(|| anyhow!("skipped preflight record lacks report"))?;
                    print_report(report);
                }
            }
        }
    }

    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("GPU preflight: FAILED");
        eprintln!("{error:#}");
        eprintln!("{USAGE}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_cli_args_defaults() {
        let parsed = parse_cli_args(Vec::<String>::new()).expect("defaults should parse");
        assert_eq!(
            parsed,
            CliCommand::Run(CliOptions {
                backend_override: None,
                run_smoke_test: true,
                force_software_fallback: false,
                json_output: None,
            })
        );
    }

    #[test]
    fn test_parse_cli_args_backend_and_smoke_toggle() {
        let parsed = parse_cli_args(vec![
            "--backend".to_string(),
            "vulkan".to_string(),
            "--no-smoke".to_string(),
        ])
        .expect("arguments should parse");
        assert_eq!(
            parsed,
            CliCommand::Run(CliOptions {
                backend_override: Some("vulkan".to_string()),
                run_smoke_test: false,
                force_software_fallback: false,
                json_output: None,
            })
        );
    }

    #[test]
    fn test_parse_cli_args_software_flag() {
        let parsed =
            parse_cli_args(vec!["--software".to_string()]).expect("software flag should parse");
        assert_eq!(
            parsed,
            CliCommand::Run(CliOptions {
                backend_override: None,
                run_smoke_test: true,
                force_software_fallback: true,
                json_output: None,
            })
        );

        let parsed = parse_cli_args(vec!["--force-fallback".to_string()])
            .expect("fallback alias should parse");
        assert_eq!(
            parsed,
            CliCommand::Run(CliOptions {
                backend_override: None,
                run_smoke_test: true,
                force_software_fallback: true,
                json_output: None,
            })
        );
    }

    #[test]
    fn test_parse_cli_args_help() {
        let parsed =
            parse_cli_args(vec!["--help".to_string()]).expect("help should parse successfully");
        assert_eq!(parsed, CliCommand::Help);
    }

    #[test]
    fn test_parse_cli_args_json_output() {
        let parsed = parse_cli_args(vec!["--json-output=artifact.json".to_string()])
            .expect("JSON output path should parse");
        assert_eq!(
            parsed,
            CliCommand::Run(CliOptions {
                backend_override: None,
                run_smoke_test: true,
                force_software_fallback: false,
                json_output: Some(PathBuf::from("artifact.json")),
            })
        );
    }

    #[test]
    fn test_parse_cli_args_rejects_unknown_flag() {
        let error = parse_cli_args(vec!["--not-a-real-flag".to_string()]).expect_err("must reject");
        assert!(
            error
                .to_string()
                .contains("unknown argument: --not-a-real-flag"),
            "unexpected message: {error}"
        );
    }
}
