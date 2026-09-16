//! Software (fallback) WGSL adapter selection.
//!
//! This module centralises how `flexpart-gpu` requests a `wgpu` adapter so that
//! development machines without a hardware GPU can still run the real
//! WGSL compute path through a software rasterizer (Mesa Lavapipe / LLVMpipe
//! on Linux, D3D12 WARP on Windows).
//!
//! The software adapter executes the same WGSL shaders as hardware. It must
//! not be confused with a CPU reference implementation that bypasses the
//! shaders. Wall-clock timings measured on a software adapter must never be
//! reported as GPU performance values.
//!
//! Selection precedence for `force_fallback_adapter`:
//! 1. explicit [`GpuAdapterOptions::force_software_fallback`] passed by the caller,
//! 2. `FLEXPART_GPU_SOFTWARE` environment variable (primary project toggle),
//! 3. `WGPU_FORCE_FALLBACK_ADAPTER` environment variable (alias for users
//!    familiar with `wgpu` naming).
//!
//! A truthy value is one of `1`, `true`, `yes`, `on` (case-insensitive,
//! surrounding whitespace ignored).

/// Environment variable requesting the software fallback adapter (primary).
pub const SOFTWARE_ADAPTER_ENV: &str = "FLEXPART_GPU_SOFTWARE";

/// Alias environment variable requesting the software fallback adapter.
pub const SOFTWARE_ADAPTER_ENV_ALIAS: &str = "WGPU_FORCE_FALLBACK_ADAPTER";

/// Options controlling `wgpu` adapter selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuAdapterOptions {
    /// When `true`, request the software fallback adapter
    /// (`force_fallback_adapter: true`).
    pub force_software_fallback: bool,
    /// Optional backend override forwarded through `WGPU_BACKEND`.
    /// `None` leaves backend selection to `wgpu` defaults.
    pub backend_override: Option<String>,
}

impl Default for GpuAdapterOptions {
    fn default() -> Self {
        Self {
            force_software_fallback: false,
            backend_override: None,
        }
    }
}

impl GpuAdapterOptions {
    /// Create options that force the software fallback adapter.
    #[must_use]
    pub const fn software() -> Self {
        Self {
            force_software_fallback: true,
            backend_override: None,
        }
    }

    /// Create options that request a hardware adapter (default behaviour).
    #[must_use]
    pub const fn hardware() -> Self {
        Self {
            force_software_fallback: false,
            backend_override: None,
        }
    }

    /// Resolve options from the process environment.
    ///
    /// Reads [`SOFTWARE_ADAPTER_ENV`] and [`SOFTWARE_ADAPTER_ENV_ALIAS`].
    /// Either variable set to a truthy value enables the fallback adapter.
    #[must_use]
    pub fn from_env() -> Self {
        Self {
            force_software_fallback: is_software_adapter_requested_from_env(),
            backend_override: None,
        }
    }

    /// Build the `wgpu` request options for these settings.
    #[must_use]
    pub const fn to_request_adapter_options(&self) -> wgpu::RequestAdapterOptions<'static> {
        wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: None,
            force_fallback_adapter: self.force_software_fallback,
        }
    }
}

/// Check whether the process environment requests the software adapter.
///
/// Returns `true` when either [`SOFTWARE_ADAPTER_ENV`] or
/// [`SOFTWARE_ADAPTER_ENV_ALIAS`] holds a truthy value.
#[must_use]
pub fn is_software_adapter_requested_from_env() -> bool {
    env_flag_truthy(SOFTWARE_ADAPTER_ENV) || env_flag_truthy(SOFTWARE_ADAPTER_ENV_ALIAS)
}

/// Check whether a `wgpu` adapter is a software (CPU) adapter.
///
/// Software rasterizers such as Lavapipe and WARP report
/// [`wgpu::DeviceType::Cpu`].
#[must_use]
pub const fn is_software_adapter(info: &wgpu::AdapterInfo) -> bool {
    matches!(info.device_type, wgpu::DeviceType::Cpu)
}

/// Parse a truthy flag value (`1`, `true`, `yes`, `on`, case-insensitive).
#[must_use]
pub fn parse_truthy_flag(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

fn env_flag_truthy(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| parse_truthy_flag(&value))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_truthy_flag_accepts_documented_values() {
        for value in ["1", "true", "TRUE", " True ", "yes", "YES", "on", "ON"] {
            assert!(parse_truthy_flag(value), "expected truthy: {value}");
        }
    }

    #[test]
    fn test_parse_truthy_flag_rejects_other_values() {
        for value in ["", "0", "false", "no", "off", "2", "auto"] {
            assert!(!parse_truthy_flag(value), "expected falsy: {value}");
        }
    }

    #[test]
    fn test_software_options_request_fallback_adapter() {
        let options = GpuAdapterOptions::software();
        assert!(options.force_software_fallback);
        assert!(options.to_request_adapter_options().force_fallback_adapter);
    }

    #[test]
    fn test_hardware_options_do_not_request_fallback_adapter() {
        let options = GpuAdapterOptions::hardware();
        assert!(!options.force_software_fallback);
        assert!(!options.to_request_adapter_options().force_fallback_adapter);
    }

    #[test]
    fn test_from_env_reads_primary_and_alias_variables() {
        // Single sequential test to avoid parallel interference on process env.
        std::env::remove_var(SOFTWARE_ADAPTER_ENV);
        std::env::remove_var(SOFTWARE_ADAPTER_ENV_ALIAS);
        assert!(!GpuAdapterOptions::from_env().force_software_fallback);

        std::env::set_var(SOFTWARE_ADAPTER_ENV, "1");
        assert!(GpuAdapterOptions::from_env().force_software_fallback);
        std::env::remove_var(SOFTWARE_ADAPTER_ENV);

        std::env::set_var(SOFTWARE_ADAPTER_ENV_ALIAS, "true");
        assert!(GpuAdapterOptions::from_env().force_software_fallback);
        std::env::remove_var(SOFTWARE_ADAPTER_ENV_ALIAS);

        assert!(!GpuAdapterOptions::from_env().force_software_fallback);
    }
}
