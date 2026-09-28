//! GPU runtime preflight checks for container/dev/CI environments.
//!
//! This module validates that wgpu can discover an adapter, request a device,
//! and execute a tiny compute workload.

use std::sync::mpsc;

use bytemuck::{Pod, Zeroable};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use wgpu::util::DeviceExt;

use super::adapter::{is_software_adapter_requested_from_env, GpuAdapterOptions};
use super::evidence::{GpuAdapterEvidence, GpuEvidenceSchema, GpuExecutionStatus};
use super::{GpuContext, GpuError};

/// Stable identifier for machine-readable GPU preflight records.
pub const GPU_PREFLIGHT_SCHEMA_ID: &str = "flexpart-gpu.gpu-preflight";

/// Current machine-readable GPU preflight schema version.
pub const GPU_PREFLIGHT_SCHEMA_VERSION: u32 = 1;

const SMOKE_MULTIPLICAND: u32 = 7;
const SMOKE_MULTIPLIER: u32 = 6;
const SMOKE_ADDEND: u32 = 5;
const SMOKE_EXPECTED_VALUE: u32 = SMOKE_MULTIPLICAND * SMOKE_MULTIPLIER + SMOKE_ADDEND;
const SMOKE_SHADER: &str = include_str!("../shaders/gpu_contract_smoke.wgsl");

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
struct SmokeInput {
    multiplicand: u32,
    multiplier: u32,
    addend: u32,
    padding: u32,
}

/// Runtime options for the GPU preflight probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuPreflightOptions {
    /// Optional backend override (`vulkan`, `metal`, `dx12`, `gl`, `webgpu`, or `auto`).
    pub backend_override: Option<String>,
    /// If true, run the tiny compute smoke test.
    pub run_smoke_test: bool,
    /// If true, request the software fallback adapter (`force_fallback_adapter`).
    ///
    /// The fallback path still runs the real WGSL compute shader. Timings on
    /// such adapters must not be reported as GPU performance values.
    pub force_software_fallback: bool,
}

impl GpuPreflightOptions {
    /// Resolve options from the process environment.
    ///
    /// Reads `WGPU_BACKEND` for the backend selector and
    /// `FLEXPART_GPU_SOFTWARE` / `WGPU_FORCE_FALLBACK_ADAPTER` for the
    /// software fallback request.
    #[must_use]
    pub fn from_env() -> Self {
        let backend_override = std::env::var("WGPU_BACKEND").ok();
        Self {
            backend_override,
            run_smoke_test: true,
            force_software_fallback: is_software_adapter_requested_from_env(),
        }
    }

    /// Whether a software fallback adapter is requested either explicitly or
    /// through `FLEXPART_GPU_SOFTWARE` / `WGPU_FORCE_FALLBACK_ADAPTER`.
    #[must_use]
    pub fn software_fallback_requested(&self) -> bool {
        self.force_software_fallback || is_software_adapter_requested_from_env()
    }
}

impl Default for GpuPreflightOptions {
    fn default() -> Self {
        Self {
            backend_override: None,
            run_smoke_test: true,
            force_software_fallback: false,
        }
    }
}

/// Compact subset of useful `wgpu::Limits` values for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceLimitsSummary {
    /// Maximum bind groups supported by the selected device.
    pub max_bind_groups: u32,
    /// Maximum storage buffers available to one shader stage.
    pub max_storage_buffers_per_shader_stage: u32,
    /// Maximum invocations permitted in one compute workgroup.
    pub max_compute_invocations_per_workgroup: u32,
    /// Maximum compute workgroup size in the X dimension.
    pub max_compute_workgroup_size_x: u32,
    /// Maximum compute workgroup size in the Y dimension.
    pub max_compute_workgroup_size_y: u32,
    /// Maximum compute workgroup size in the Z dimension.
    pub max_compute_workgroup_size_z: u32,
    /// Maximum workgroup count along one dispatch dimension.
    pub max_compute_workgroups_per_dimension: u32,
    /// Maximum buffer allocation size in bytes.
    pub max_buffer_size: u64,
}

impl From<wgpu::Limits> for DeviceLimitsSummary {
    fn from(value: wgpu::Limits) -> Self {
        Self {
            max_bind_groups: value.max_bind_groups,
            max_storage_buffers_per_shader_stage: value.max_storage_buffers_per_shader_stage,
            max_compute_invocations_per_workgroup: value.max_compute_invocations_per_workgroup,
            max_compute_workgroup_size_x: value.max_compute_workgroup_size_x,
            max_compute_workgroup_size_y: value.max_compute_workgroup_size_y,
            max_compute_workgroup_size_z: value.max_compute_workgroup_size_z,
            max_compute_workgroups_per_dimension: value.max_compute_workgroups_per_dimension,
            max_buffer_size: value.max_buffer_size,
        }
    }
}

/// H2D, WGSL arithmetic, and D2H evidence from the smoke computation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuSmokeTestEvidence {
    /// Whether the smoke calculation passed or was explicitly skipped.
    pub status: GpuExecutionStatus,
    /// Host values uploaded to the uniform input buffer.
    pub input_values: [u32; 3],
    /// Expected result of `multiplicand * multiplier + addend`.
    pub expected_value: u32,
    /// Value read back from the GPU, absent only when the smoke was skipped.
    pub actual_value: Option<u32>,
}

/// Structured machine-readable report produced by a successful preflight run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuPreflightReport {
    /// Preflight schema identity.
    pub schema: GpuEvidenceSchema,
    /// Normalized backend selector requested by the caller.
    pub requested_backend: String,
    /// Selected adapter provenance and hardware/software classification.
    pub adapter: GpuAdapterEvidence,
    /// Device limits relevant to calculative GPU kernels.
    pub limits: DeviceLimitsSummary,
    /// Whether filterable f32 texture sampling is supported for wind resources.
    pub supports_wind_texture_sampling: bool,
    /// Explicit smoke execution evidence.
    pub smoke_test: GpuSmokeTestEvidence,
}

/// Machine-readable outcome that preserves failed preflight attempts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GpuPreflightRecord {
    /// Preflight schema identity.
    pub schema: GpuEvidenceSchema,
    /// Overall initialization/smoke execution status.
    pub status: GpuExecutionStatus,
    /// Requested backend before adapter initialization.
    pub requested_backend: String,
    /// Initialized adapter report, absent on failure and present on pass/skip.
    pub report: Option<GpuPreflightReport>,
    /// Explicit error detail, present only on failure.
    pub failure: Option<String>,
    /// Explicit reason the smoke calculation did not execute.
    pub skip_reason: Option<String>,
}

impl GpuPreflightRecord {
    /// Validate the exact pass/fail/skip invariants of a preflight record.
    ///
    /// # Errors
    ///
    /// Returns [`GpuPreflightError::InvalidRecord`] when schema, status,
    /// provenance, smoke result, or reason fields contradict one another.
    pub fn validate(&self) -> Result<(), GpuPreflightError> {
        if self.schema.id != GPU_PREFLIGHT_SCHEMA_ID
            || self.schema.version != GPU_PREFLIGHT_SCHEMA_VERSION
        {
            return Err(GpuPreflightError::InvalidRecord(
                "unsupported preflight schema".to_string(),
            ));
        }
        if self.requested_backend.trim().is_empty() {
            return Err(GpuPreflightError::InvalidRecord(
                "requested backend is empty".to_string(),
            ));
        }

        match self.status {
            GpuExecutionStatus::Passed => {
                let report = self.require_report("passing record requires a report")?;
                if self.failure.is_some() || self.skip_reason.is_some() {
                    return Err(GpuPreflightError::InvalidRecord(
                        "passing record cannot contain failure or skip reason".to_string(),
                    ));
                }
                validate_preflight_report(report, self, GpuExecutionStatus::Passed)?;
            }
            GpuExecutionStatus::Failed => {
                if self.report.is_some()
                    || self
                        .failure
                        .as_deref()
                        .is_none_or(|failure| failure.trim().is_empty())
                    || self.skip_reason.is_some()
                {
                    return Err(GpuPreflightError::InvalidRecord(
                        "failed record requires only a non-empty failure".to_string(),
                    ));
                }
            }
            GpuExecutionStatus::Skipped => {
                let report = self.require_report("skipped record requires a report")?;
                if self.failure.is_some()
                    || self
                        .skip_reason
                        .as_deref()
                        .is_none_or(|reason| reason.trim().is_empty())
                {
                    return Err(GpuPreflightError::InvalidRecord(
                        "skipped record requires only a non-empty skip reason".to_string(),
                    ));
                }
                validate_preflight_report(report, self, GpuExecutionStatus::Skipped)?;
            }
        }
        Ok(())
    }

    fn require_report(
        &self,
        message: &'static str,
    ) -> Result<&GpuPreflightReport, GpuPreflightError> {
        self.report
            .as_ref()
            .ok_or_else(|| GpuPreflightError::InvalidRecord(message.to_string()))
    }
}

/// Errors produced while selecting a backend, initializing wgpu, executing the
/// smoke calculation, or validating its machine-readable record.
#[derive(Debug, Error)]
pub enum GpuPreflightError {
    #[error("invalid backend selector `{selector}`: {reason}")]
    InvalidBackendSelector { selector: String, reason: String },
    #[error("no suitable GPU adapter found (requested backend: {requested_backend})")]
    NoAdapter { requested_backend: String },
    #[error("failed to request GPU device: {0}")]
    DeviceRequest(#[from] wgpu::RequestDeviceError),
    #[error("buffer map operation failed: {0}")]
    BufferMap(#[from] wgpu::BufferAsyncError),
    #[error("buffer map callback channel closed unexpectedly")]
    MapChannelClosed,
    #[error("compute smoke test mismatch: expected 0x{expected:08x}, got 0x{actual:08x}")]
    SmokeTestMismatch { expected: u32, actual: u32 },
    #[error("GPU smoke operation failed: {0}")]
    GpuOperation(String),
    #[error("invalid GPU preflight record: {0}")]
    InvalidRecord(String),
}

/// Parse and normalize a backend selector (`auto` or comma-separated backend list).
///
/// # Errors
///
/// Returns [`GpuPreflightError::InvalidBackendSelector`] when the selector is
/// empty or contains a backend name that wgpu does not support here.
pub fn normalize_backend_selector(selector: &str) -> Result<String, GpuPreflightError> {
    let trimmed = selector.trim().to_lowercase();
    if trimmed.is_empty() {
        return Err(GpuPreflightError::InvalidBackendSelector {
            selector: selector.to_string(),
            reason: "value cannot be empty".to_string(),
        });
    }

    if trimmed == "auto" || trimmed == "all" {
        return Ok("auto".to_string());
    }

    let mut normalized_tokens: Vec<String> = Vec::new();
    for token in trimmed.split(',') {
        let token = token.trim();
        let canonical = match token {
            "vulkan" => "vulkan",
            "metal" => "metal",
            "dx12" | "d3d12" => "dx12",
            "gl" | "opengl" => "gl",
            "webgpu" | "browser_webgpu" => "webgpu",
            "" => {
                return Err(GpuPreflightError::InvalidBackendSelector {
                    selector: selector.to_string(),
                    reason: "contains an empty backend token".to_string(),
                });
            }
            _ => {
                return Err(GpuPreflightError::InvalidBackendSelector {
                    selector: selector.to_string(),
                    reason: format!(
                        "unsupported backend `{token}` (allowed: auto,vulkan,metal,dx12,gl,webgpu)"
                    ),
                });
            }
        };

        if !normalized_tokens.iter().any(|value| value == canonical) {
            normalized_tokens.push(canonical.to_string());
        }
    }

    if normalized_tokens.is_empty() {
        return Err(GpuPreflightError::InvalidBackendSelector {
            selector: selector.to_string(),
            reason: "no usable backend tokens".to_string(),
        });
    }

    Ok(normalized_tokens.join(","))
}

fn resolve_requested_backend(options: &GpuPreflightOptions) -> Result<String, GpuPreflightError> {
    if let Some(cli_value) = options.backend_override.as_deref() {
        return normalize_backend_selector(cli_value);
    }
    if let Ok(env_value) = std::env::var("WGPU_BACKEND") {
        return normalize_backend_selector(&env_value);
    }
    Ok("auto".to_string())
}

/// Run GPU preflight:
/// 1. discover adapter and create device via wgpu
/// 2. collect adapter/device diagnostics
/// 3. run tiny compute smoke test (optional)
///
/// The software fallback adapter can be requested explicitly through
/// [`GpuPreflightOptions::force_software_fallback`] or through the
/// `FLEXPART_GPU_SOFTWARE` / `WGPU_FORCE_FALLBACK_ADAPTER` environment
/// variables. The fallback still executes the real WGSL compute shader.
///
/// # Errors
///
/// Returns an error when backend selection, adapter/device initialization, the
/// compute dispatch, synchronization, readback, or result verification fails.
pub async fn run_preflight(
    options: GpuPreflightOptions,
) -> Result<GpuPreflightReport, GpuPreflightError> {
    let requested_backend = resolve_requested_backend(&options)?;
    let fallback_requested = options.software_fallback_requested();
    let backend_was_explicit =
        options.backend_override.is_some() || std::env::var_os("WGPU_BACKEND").is_some();
    let adapter_options = GpuAdapterOptions {
        force_software_fallback: fallback_requested,
        backend_override: backend_was_explicit.then(|| requested_backend.clone()),
    };
    let context = match GpuContext::with_options(adapter_options).await {
        Ok(context) => context,
        Err(GpuError::NoAdapter) => {
            return Err(GpuPreflightError::NoAdapter { requested_backend });
        }
        Err(GpuError::DeviceRequest(error)) => {
            return Err(GpuPreflightError::DeviceRequest(error));
        }
        Err(error) => return Err(GpuPreflightError::GpuOperation(error.to_string())),
    };

    let smoke_test = if options.run_smoke_test {
        let actual_value = run_compute_smoke_test(&context).await?;
        GpuSmokeTestEvidence {
            status: GpuExecutionStatus::Passed,
            input_values: [SMOKE_MULTIPLICAND, SMOKE_MULTIPLIER, SMOKE_ADDEND],
            expected_value: SMOKE_EXPECTED_VALUE,
            actual_value: Some(actual_value),
        }
    } else {
        GpuSmokeTestEvidence {
            status: GpuExecutionStatus::Skipped,
            input_values: [SMOKE_MULTIPLICAND, SMOKE_MULTIPLIER, SMOKE_ADDEND],
            expected_value: SMOKE_EXPECTED_VALUE,
            actual_value: None,
        }
    };

    Ok(GpuPreflightReport {
        schema: preflight_schema(),
        requested_backend,
        adapter: GpuAdapterEvidence::from_context(&context),
        limits: DeviceLimitsSummary::from(context.device.limits()),
        supports_wind_texture_sampling: context.supports_wind_texture_sampling(),
        smoke_test,
    })
}

/// Run preflight while retaining a serializable failure record.
///
/// Unlike [`run_preflight`], this function never discards initialization or
/// smoke failures. Callers must still fail their gate unless `status` is
/// [`GpuExecutionStatus::Passed`].
pub async fn run_preflight_record(options: GpuPreflightOptions) -> GpuPreflightRecord {
    let requested_backend = resolve_requested_backend(&options).unwrap_or_else(|_| {
        options
            .backend_override
            .clone()
            .or_else(|| std::env::var("WGPU_BACKEND").ok())
            .unwrap_or_else(|| "auto".to_string())
    });
    match run_preflight(options).await {
        Ok(report) => {
            let status = report.smoke_test.status;
            let skip_reason = (status == GpuExecutionStatus::Skipped)
                .then(|| "smoke test disabled by request".to_string());
            GpuPreflightRecord {
                schema: preflight_schema(),
                status,
                requested_backend,
                report: Some(report),
                failure: None,
                skip_reason,
            }
        }
        Err(error) => GpuPreflightRecord {
            schema: preflight_schema(),
            status: GpuExecutionStatus::Failed,
            requested_backend,
            report: None,
            failure: Some(error.to_string()),
            skip_reason: None,
        },
    }
}

async fn run_compute_smoke_test(context: &GpuContext) -> Result<u32, GpuPreflightError> {
    context.device.push_error_scope(wgpu::ErrorFilter::Internal);
    context
        .device
        .push_error_scope(wgpu::ErrorFilter::OutOfMemory);
    context
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);

    let result = run_compute_smoke_test_scoped(context);
    let validation_error = context.device.pop_error_scope().await;
    let out_of_memory_error = context.device.pop_error_scope().await;
    let internal_error = context.device.pop_error_scope().await;
    if let Some(error) = validation_error.or(out_of_memory_error).or(internal_error) {
        return Err(GpuPreflightError::GpuOperation(error.to_string()));
    }
    result
}

fn run_compute_smoke_test_scoped(context: &GpuContext) -> Result<u32, GpuPreflightError> {
    let (input_buffer, output_buffer, staging_buffer) = create_smoke_buffers(context);
    let (pipeline, bind_group) = create_smoke_pipeline(context, &input_buffer, &output_buffer);
    submit_smoke_dispatch(
        context,
        &pipeline,
        &bind_group,
        &output_buffer,
        &staging_buffer,
    );
    let value = read_smoke_result(context, &staging_buffer)?;
    if value != SMOKE_EXPECTED_VALUE {
        return Err(GpuPreflightError::SmokeTestMismatch {
            expected: SMOKE_EXPECTED_VALUE,
            actual: value,
        });
    }

    Ok(value)
}

fn create_smoke_buffers(context: &GpuContext) -> (wgpu::Buffer, wgpu::Buffer, wgpu::Buffer) {
    let input = SmokeInput {
        multiplicand: SMOKE_MULTIPLICAND,
        multiplier: SMOKE_MULTIPLIER,
        addend: SMOKE_ADDEND,
        padding: 0,
    };
    let input_buffer = context
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gpu_preflight_input_buffer"),
            contents: bytemuck::bytes_of(&input),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
    let output_buffer = context
        .device
        .create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gpu_preflight_output_buffer"),
            contents: bytemuck::cast_slice(&[0_u32]),
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
        });

    let staging_buffer = context.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("gpu_preflight_staging_buffer"),
        size: 4,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    (input_buffer, output_buffer, staging_buffer)
}

fn create_smoke_pipeline(
    context: &GpuContext,
    input_buffer: &wgpu::Buffer,
    output_buffer: &wgpu::Buffer,
) -> (wgpu::ComputePipeline, wgpu::BindGroup) {
    let shader = context
        .device
        .create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gpu_preflight_shader"),
            source: wgpu::ShaderSource::Wgsl(SMOKE_SHADER.into()),
        });

    let bind_group_layout =
        context
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("gpu_preflight_bind_group_layout"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Storage { read_only: false },
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                ],
            });

    let bind_group = context
        .device
        .create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gpu_preflight_bind_group"),
            layout: &bind_group_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: input_buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: output_buffer.as_entire_binding(),
                },
            ],
        });

    let pipeline_layout = context
        .device
        .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gpu_preflight_pipeline_layout"),
            bind_group_layouts: &[&bind_group_layout],
            push_constant_ranges: &[],
        });
    let pipeline = context
        .device
        .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("gpu_preflight_pipeline"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: wgpu::PipelineCompilationOptions::default(),
            cache: None,
        });

    (pipeline, bind_group)
}

fn submit_smoke_dispatch(
    context: &GpuContext,
    pipeline: &wgpu::ComputePipeline,
    bind_group: &wgpu::BindGroup,
    output_buffer: &wgpu::Buffer,
    staging_buffer: &wgpu::Buffer,
) {
    let mut encoder = context
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gpu_preflight_encoder"),
        });

    {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("gpu_preflight_compute_pass"),
            timestamp_writes: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, bind_group, &[]);
        pass.dispatch_workgroups(1, 1, 1);
    }

    encoder.copy_buffer_to_buffer(output_buffer, 0, staging_buffer, 0, 4);
    context.queue.submit(Some(encoder.finish()));
    let _ = context.device.poll(wgpu::Maintain::Wait);
}

fn read_smoke_result(
    context: &GpuContext,
    staging_buffer: &wgpu::Buffer,
) -> Result<u32, GpuPreflightError> {
    let slice = staging_buffer.slice(..);
    let (tx, rx) = mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    let _ = context.device.poll(wgpu::Maintain::Wait);

    let map_result = rx.recv().map_err(|_| GpuPreflightError::MapChannelClosed)?;
    map_result?;

    let mapped = slice.get_mapped_range();
    let mut raw = [0_u8; 4];
    raw.copy_from_slice(&mapped[..4]);
    drop(mapped);
    staging_buffer.unmap();

    Ok(u32::from_le_bytes(raw))
}

fn preflight_schema() -> GpuEvidenceSchema {
    GpuEvidenceSchema {
        id: GPU_PREFLIGHT_SCHEMA_ID.to_string(),
        version: GPU_PREFLIGHT_SCHEMA_VERSION,
    }
}

fn validate_preflight_report(
    report: &GpuPreflightReport,
    record: &GpuPreflightRecord,
    expected_status: GpuExecutionStatus,
) -> Result<(), GpuPreflightError> {
    if report.schema.id != GPU_PREFLIGHT_SCHEMA_ID
        || report.schema.version != GPU_PREFLIGHT_SCHEMA_VERSION
        || report.requested_backend != record.requested_backend
    {
        return Err(GpuPreflightError::InvalidRecord(
            "report schema/backend does not match its record".to_string(),
        ));
    }
    report
        .adapter
        .validate()
        .map_err(|error| GpuPreflightError::InvalidRecord(error.to_string()))?;
    let [multiplicand, multiplier, addend] = report.smoke_test.input_values;
    let derived_expected = multiplicand.wrapping_mul(multiplier).wrapping_add(addend);
    if report.smoke_test.expected_value != derived_expected {
        return Err(GpuPreflightError::InvalidRecord(
            "smoke expected value does not match its recorded inputs".to_string(),
        ));
    }
    if report.smoke_test.status != expected_status {
        return Err(GpuPreflightError::InvalidRecord(
            "record and smoke statuses differ".to_string(),
        ));
    }

    match expected_status {
        GpuExecutionStatus::Passed => {
            let actual = report.smoke_test.actual_value.ok_or_else(|| {
                GpuPreflightError::InvalidRecord(
                    "passing smoke record lacks an actual value".to_string(),
                )
            })?;
            if actual != report.smoke_test.expected_value {
                return Err(GpuPreflightError::InvalidRecord(
                    "passing smoke result does not match its expected value".to_string(),
                ));
            }
        }
        GpuExecutionStatus::Skipped => {
            if report.smoke_test.actual_value.is_some() {
                return Err(GpuPreflightError::InvalidRecord(
                    "skipped smoke record cannot contain an actual value".to_string(),
                ));
            }
        }
        GpuExecutionStatus::Failed => {
            return Err(GpuPreflightError::InvalidRecord(
                "failed smoke details belong in the record failure".to_string(),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backend_selector_normalizes_auto() {
        assert_eq!(
            normalize_backend_selector("  ALL ").expect("all is accepted"),
            "auto"
        );
        assert_eq!(
            normalize_backend_selector("auto").expect("auto is accepted"),
            "auto"
        );
    }

    #[test]
    fn test_backend_selector_normalizes_and_deduplicates() {
        assert_eq!(
            normalize_backend_selector("vulkan, gl, vulkan").expect("valid selector"),
            "vulkan,gl"
        );
    }

    #[test]
    fn test_backend_selector_rejects_unknown_token() {
        let error = normalize_backend_selector("cuda").expect_err("cuda is not a wgpu backend");
        let rendered = error.to_string();
        assert!(
            rendered.contains("unsupported backend"),
            "unexpected message: {rendered}"
        );
    }

    #[test]
    fn test_preflight_smoke_runs_if_adapter_available() {
        let instance = wgpu::Instance::default();
        let has_adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            }))
            .is_some();

        if !has_adapter {
            return;
        }

        let report = pollster::block_on(run_preflight(GpuPreflightOptions::default()))
            .expect("preflight should succeed when adapter exists");
        assert!(!report.adapter.name.is_empty());
        assert_eq!(
            report.smoke_test.actual_value,
            Some(SMOKE_EXPECTED_VALUE),
            "smoke test should calculate the expected value"
        );
        assert_eq!(report.smoke_test.status, GpuExecutionStatus::Passed);
    }

    #[test]
    fn test_software_fallback_requested_combines_flag_and_env() {
        std::env::remove_var(super::super::adapter::SOFTWARE_ADAPTER_ENV);
        std::env::remove_var(super::super::adapter::SOFTWARE_ADAPTER_ENV_ALIAS);
        let explicit = GpuPreflightOptions {
            backend_override: None,
            run_smoke_test: true,
            force_software_fallback: true,
        };
        assert!(explicit.software_fallback_requested());

        let implicit = GpuPreflightOptions::default();
        assert!(!implicit.software_fallback_requested());

        std::env::set_var(super::super::adapter::SOFTWARE_ADAPTER_ENV, "1");
        assert!(implicit.software_fallback_requested());
        std::env::remove_var(super::super::adapter::SOFTWARE_ADAPTER_ENV);
    }

    #[test]
    fn test_preflight_invalid_backend_serializes_failed_record() {
        let record = pollster::block_on(run_preflight_record(GpuPreflightOptions {
            backend_override: Some("not-a-backend".to_string()),
            run_smoke_test: true,
            force_software_fallback: false,
        }));

        assert_eq!(record.status, GpuExecutionStatus::Failed);
        assert!(record.report.is_none());
        assert!(record.failure.is_some());
        assert!(record.skip_reason.is_none());
        record
            .validate()
            .expect("failure record is structurally valid");
        let json = serde_json::to_value(&record).expect("failure record serializes");
        assert_eq!(json["schema"]["id"], GPU_PREFLIGHT_SCHEMA_ID);
        assert_eq!(json["status"], "failed");

        let mut dishonest = record;
        dishonest.status = GpuExecutionStatus::Passed;
        assert!(matches!(
            dishonest.validate(),
            Err(GpuPreflightError::InvalidRecord(_))
        ));
    }

    #[test]
    fn test_preflight_disabled_smoke_serializes_skipped_record() {
        let instance = wgpu::Instance::default();
        let has_adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: None,
                force_fallback_adapter: false,
            }))
            .is_some();
        if !has_adapter {
            return;
        }

        let record = pollster::block_on(run_preflight_record(GpuPreflightOptions {
            run_smoke_test: false,
            ..GpuPreflightOptions::default()
        }));

        assert_eq!(record.status, GpuExecutionStatus::Skipped);
        assert!(record.report.is_some());
        assert!(record.failure.is_none());
        assert_eq!(
            record.skip_reason.as_deref(),
            Some("smoke test disabled by request")
        );
        record
            .validate()
            .expect("skip record is structurally valid");
    }
}
