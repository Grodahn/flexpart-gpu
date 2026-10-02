// W/interface-to-shared-grid remap kernel for issue #88.
//
// Ported from the #80 two-stage production path
// (`verttransform_mod.f90:544-545,607-622` remap plus
// `interpol_mod.f90:1022-1027,1651-1703` meter-coordinate sampling at pinned
// revision `c70586c2b7f5258850705325881c61f557ea9bd8`) as implemented by
// `src/meteorology/vertical_sampling.rs::remap_interface_motion_to_model_grid`.
//
// Execution order:
//
// ```text
// canonical W/interface source (interface heights + values, level heights)
// -> this kernel: interface values remapped onto the shared
//    [ground, model levels] height grid (ground/top pinned,
//    strict-interior interpolated with the identical METRE-mode primitive)
// -> canonical shared vertical representation (shared heights + remapped values)
// -> vertical_sample.wgsl: ordinary particle-height sample
// ```
//
// Direct interpolation on W/interface heights is deliberately not production
// behavior. This stage must not be collapsed into the sample stage: boundary
// pinning, staggering, and floating-point order would change.
//
// Host owns validation: nz >= 2, interface len nz+1 strictly increasing with
// [0] == 0.0, level len nz strictly increasing, finite values, interior shared
// heights strictly inside the interface domain, top compatibility, and the #80
// provenance contract (single column, hybrid-interface/height/wzlev algorithm
// ids, ASL terrain). Supported remap arithmetic executes here in f32.
//
// Units:
// - heights: [m] AGL, physical bottom-to-top
// - values/outputs: [m/s] geometric vertical velocity, positive upward
//
// Buffer contract:
// - binding(0): interface heights AGL, array<f32> [nz+1], read-only, [0] == 0.0
// - binding(1): interface values, array<f32> [nz+1], read-only
// - binding(2): level heights AGL, array<f32> [nz], read-only, physical bottom-to-top
// - binding(3): remapped shared values, array<f32> [nz+1], read-write
//   (GPU-resident shared representation for the later sample stage;
//   shared heights [ground=0.0, level heights] are host-uploaded geometry)
// - binding(4): uniform params (model_level_count nz, pad, pad, pad)

struct VerticalRemapParams {
    model_level_count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

@group(0) @binding(0)
var<storage, read> interface_heights: array<f32>;

@group(0) @binding(1)
var<storage, read> interface_values: array<f32>;

@group(0) @binding(2)
var<storage, read> level_heights: array<f32>;

@group(0) @binding(3)
var<storage, read_write> remapped_values: array<f32>;

@group(0) @binding(4)
var<uniform> params: VerticalRemapParams;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let nz = params.model_level_count;
    // Shared grid has nz+1 entries: [ground, nz model levels].
    let shared_count = nz + 1u;
    let idx = gid.y * (nwg.x * 64u) + gid.x;
    if (idx >= shared_count) {
        return;
    }
    let interface_count = nz + 1u;
    if (idx == 0u) {
        // Ground boundary is pinned directly, matching
        // `remap_interface_motion_to_model_grid` model_grid[0].
        remapped_values[idx] = interface_values[0];
        return;
    }
    if (idx == nz) {
        // Top shared model height is pinned to the interface top value,
        // not interpolated, matching the #80 production path.
        remapped_values[idx] = interface_values[interface_count - 1u];
        return;
    }
    // Strict-interior shared height: level_heights[idx-1] in physical
    // bottom-to-top order. Host guarantees 0.0 < h < interface top.
    // Reproduce the identical METRE-mode primitive, preserving order.
    let h = level_heights[idx - 1u];
    let lowest_height = interface_heights[0];
    let lowest_value = interface_values[0];
    let last_interface = interface_count - 1u;
    let highest_height = interface_heights[last_interface];
    let highest_value = interface_values[last_interface];
    if (h <= lowest_height) {
        remapped_values[idx] = lowest_value;
        return;
    }
    if (h >= highest_height) {
        remapped_values[idx] = highest_value;
        return;
    }
    var upper = 1u;
    while (upper < interface_count && interface_heights[upper] <= h) {
        upper += 1u;
    }
    let lower = upper - 1u;
    let lower_height = interface_heights[lower];
    let upper_height = interface_heights[upper];
    let lower_value = interface_values[lower];
    let upper_value = interface_values[upper];
    let denominator = upper_height - lower_height;
    let weight_upper = (h - lower_height) / denominator;
    let weight_lower = (upper_height - h) / denominator;
    remapped_values[idx] = lower_value * weight_lower + upper_value * weight_upper;
}
