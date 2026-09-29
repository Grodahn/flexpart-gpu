// Canonical instantaneous temporal interpolation kernel for issue #89.
//
// Ported from FLEXPART 11.1 `interpol_mod.f90:190-198` (`find_time_vars`) and
// `interpol_mod.f90:531-537` (`temporal_interpolation`):
//   dt1 = itime - memtime(1), dt2 = memtime(2) - itime, dtt = 1 / (dt1 + dt2)
//   output = (time1*dt2 + time2*dt1) * dtt
//
// Host validation preserves the exact #74 timestamp, endpoint and fail-closed
// semantics (ordered source timestamps, exact-source behavior, first/last
// endpoints, duplicate/non-monotonic/missing/inconsistent rejection,
// unsupported extrapolation rejection). This kernel executes only the
// elementwise blend for one pre-validated bracketing pair:
//
//   output[i] = lower[i] * w_lower + upper[i] * w_upper
//
// where (w_lower, w_upper) = (dt2*dtt, dt1*dtt) are the frozen #71/#74
// temporal weights derived on the host in f64 and passed as f32 uniforms.
// Accumulated-field/reset semantics are out of scope and must not enter here.
//
// Buffer contract:
// - binding 0: lower-bracket field values, f32[element_count], read-only
// - binding 1: upper-bracket field values, f32[element_count], read-only
// - binding 2: blended output values, f32[element_count], read-write
//   (GPU-resident for #76 composition; host readback only at explicit
//   validation/output boundaries)
// - binding 3: uniform params (w_lower, w_upper, element_count, pad)

struct TemporalBlendParams {
    w_lower: f32,
    w_upper: f32,
    element_count: u32,
    _pad0: u32,
};

@group(0) @binding(0)
var<storage, read> lower_values: array<f32>;

@group(0) @binding(1)
var<storage, read> upper_values: array<f32>;

@group(0) @binding(2)
var<storage, read_write> blended_values: array<f32>;

@group(0) @binding(3)
var<uniform> params: TemporalBlendParams;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let idx = gid.y * (nwg.x * 64u) + gid.x;
    if (idx >= params.element_count) {
        return;
    }
    blended_values[idx] = lower_values[idx] * params.w_lower + upper_values[idx] * params.w_upper;
}
