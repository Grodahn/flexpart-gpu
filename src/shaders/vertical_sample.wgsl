// Vertical model-level sampling kernel for issue #88.
//
// Ported from the canonical CPU semantics in `src/meteorology/vertical_sampling.rs`
// (issue #73), which ports FLEXPART 11.1 `interpol_mod.f90:215-242`
// (`find_z_level_meters`, METRE mode), `:406-430` (`find_vert_vars_lin`,
// `log_interpol=.false.`) and `:539-547` (`vert_interpol`).
//
// Host (Rust) owns all #30/#73 validation and geometry preparation:
// monotonicity, finiteness, ordering normalization to physical bottom-to-top,
// storage layout, terrain/ASL-to-AGL resolution, provenance and fail-closed
// rejection. Supported numerical sampling executes here in WGSL f32:
//
// ```text
// if h <= heights[0] { value[0] }
// else if h >= heights[last] { value[last] }
// else {
//   find first upper with heights[upper] > h (strictly greater, FLEXPART search)
//   lower = upper - 1
//   weight_upper = (h - heights[lower]) / (heights[upper] - heights[lower])
//   weight_lower = (heights[upper] - h) / (heights[upper] - heights[lower])
//   value = values[lower] * weight_lower + values[upper] * weight_upper
// }
// ```
//
// Units:
// - heights/queries: [m] AGL, strictly increasing bottom-to-top, finite
// - values/outputs: field units (e.g. K, m/s), finite
//
// Buffer contract:
// - binding(0): grid heights, array<f32> [grid_count], read-only, physical bottom-to-top
// - binding(1): grid values, array<f32> [grid_count], read-only
// - binding(2): query heights AGL, array<f32> [query_count], read-only
// - binding(3): sampled outputs, array<f32> [query_count], read-write
//   (GPU-resident for #76; host readback only at explicit validation/output boundaries)
// - binding(4): uniform params (grid_count, query_count, pad, pad)

struct VerticalSampleParams {
    grid_count: u32,
    query_count: u32,
    _pad0: u32,
    _pad1: u32,
};

@group(0) @binding(0)
var<storage, read> grid_heights: array<f32>;

@group(0) @binding(1)
var<storage, read> grid_values: array<f32>;

@group(0) @binding(2)
var<storage, read> query_heights: array<f32>;

@group(0) @binding(3)
var<storage, read_write> sampled_outputs: array<f32>;

@group(0) @binding(4)
var<uniform> params: VerticalSampleParams;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let idx = gid.y * (nwg.x * 64u) + gid.x;
    if (idx >= params.query_count) {
        return;
    }
    let h = query_heights[idx];
    let grid_count = params.grid_count;
    // Host validates grid_count >= 2 and strictly increasing finite geometry;
    // the shader preserves the exact FLEXPART clamping and weight order.
    let lowest_height = grid_heights[0];
    let lowest_value = grid_values[0];
    let last_index = grid_count - 1u;
    let highest_height = grid_heights[last_index];
    let highest_value = grid_values[last_index];
    if (h <= lowest_height) {
        sampled_outputs[idx] = lowest_value;
        return;
    }
    if (h >= highest_height) {
        sampled_outputs[idx] = highest_value;
        return;
    }
    var upper = 1u;
    // FLEXPART first-height-strictly-above-zt search. Heights are host-validated
    // finite and strictly increasing, so this terminates strictly inside.
    while (upper < grid_count && grid_heights[upper] <= h) {
        upper += 1u;
    }
    let lower = upper - 1u;
    let lower_height = grid_heights[lower];
    let upper_height = grid_heights[upper];
    let lower_value = grid_values[lower];
    let upper_value = grid_values[upper];
    let denominator = upper_height - lower_height;
    let weight_upper = (h - lower_height) / denominator;
    let weight_lower = (upper_height - h) / denominator;
    sampled_outputs[idx] = lower_value * weight_lower + upper_value * weight_upper;
}
