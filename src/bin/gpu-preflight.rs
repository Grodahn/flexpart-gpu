use anyhow::{anyhow, Result};
use flexpart_gpu::gpu::{run_preflight, GpuPreflightOptions};

const USAGE: &str = "\
GPU runtime preflight check.

Usage:
  cargo run --bin gpu-preflight -- [--backend <value>] [--software] [--no-smoke]
  cargo run --bin gpu-preflight -- --help

Options:
  --backend <value>  Override backend selector (auto|vulkan|metal|dx12|gl|webgpu)
  --software         Request the software fallback adapter (same as FLEXPART_GPU_SOFTWARE=1).
                     Runs the real WGSL compute path on Lavapipe/WARP; timings must
                     not be used as GPU performance values.
  --force-fallback   Alias for --software
  --no-smoke         Skip tiny compute dispatch/readback smoke test
  -h, --help         Show this help

Environment:
  FLEXPART_GPU_SOFTWARE=1 or WGPU_FORCE_FALLBACK_ADAPTER=1 selects the software adapter.
";

#[derive(Debug, Clone, PartialEq, Eq)]
struct CliOptions {
    backend_override: Option<String>,
    run_smoke_test: bool,
    force_software_fallback: bool,
}

impl Default for CliOptions {
    fn default() -> Self {
        Self {
            backend_override: None,
            run_smoke_test: true,
            force_software_fallback: false,
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
            _ if argument.starts_with("--backend=") => {
                let (_, value) = argument
                    .split_once('=')
                    .ok_or_else(|| anyhow!("invalid --backend argument"))?;
                options.backend_override = Some(value.to_string());
            }
            _ => return Err(anyhow!("unknown argument: {argument}")),
        }
    }

    Ok(CliCommand::Run(options))
}

fn print_report(report: &flexpart_gpu::gpu::GpuPreflightReport) {
    println!("GPU preflight: OK");
    println!("requested backend: {}", report.requested_backend);
    println!(
        "adapter: {} ({:?}, {:?})",
        report.adapter_name, report.adapter_backend, report.adapter_type
    );
    println!("software fallback requested: {}", report.fallback_requested);
    println!("software adapter: {}", report.is_software_adapter);
    if report.is_software_adapter {
        println!("note: software WGSL adapter in use; timings must not be used as GPU performance values");
    }
    println!(
        "device ids: vendor=0x{:04x} device=0x{:04x}",
        report.vendor_id, report.device_id
    );
    println!("driver: {} | {}", report.driver, report.driver_info);
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
    if let Some(value) = report.smoke_test_value {
        println!("smoke test: PASS (0x{value:08x})");
    } else {
        println!("smoke test: SKIPPED");
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
            let report = pollster::block_on(run_preflight(GpuPreflightOptions {
                backend_override: cli.backend_override,
                run_smoke_test: cli.run_smoke_test,
                force_software_fallback: cli.force_software_fallback,
            }))?;
            print_report(&report);
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
                force_software_fallback: false
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
                force_software_fallback: false
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
                force_software_fallback: true
            })
        );

        let parsed = parse_cli_args(vec!["--force-fallback".to_string()])
            .expect("fallback alias should parse");
        assert_eq!(
            parsed,
            CliCommand::Run(CliOptions {
                backend_override: None,
                run_smoke_test: true,
                force_software_fallback: true
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
