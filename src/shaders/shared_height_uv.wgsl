// FLEXPART 11.1 verttransform_mod.f90:296-353,487-623 (eta=no).
// U/V = (lower*dz2 + upper*dz1)/(dz1+dz2); shared_heights endpoints copy native endpoints.
// Native lanes are [height AGL m, U m/s, V m/s, local surface pressure Pa].
struct Parameters { columns: u32, levels: u32, initialize: u32, padding: u32 }
const INITIAL_SELECTION_PRESSURE_PA: f32 = 100000.0;
@group(0) @binding(0) var<storage, read> native: array<vec4<f32>>;
@group(0) @binding(1) var<storage, read_write> shared_heights: array<f32>;
@group(0) @binding(2) var<storage, read_write> target_status: array<u32>;
@group(0) @binding(3) var<storage, read_write> output_u: array<f32>;
@group(0) @binding(4) var<storage, read_write> output_v: array<f32>;
@group(0) @binding(5) var<storage, read_write> status: array<u32>;
@group(2) @binding(0) var<uniform> params: Parameters;

fn finite(v: f32) -> bool { return abs(v) <= 3.402823466e38; }

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let column = id.x;
    if (params.initialize != 0u) {
        if (column != 0u) { return; }
        target_status[0] = 1u;
        // Accepted regional mother route selects the first point in the pinned y/x scan.
        if (!finite(native[0].w) || native[0].w <= INITIAL_SELECTION_PRESSURE_PA) { target_status[0] = 3u; return; }
        var previous = -1.0;
        for (var level = 0u; level < params.levels; level++) {
            let height = native[level*params.columns].x;
            if (!finite(height) || height <= previous || (level == 0u && height != 0.0)) {
                target_status[0] = 3u; return;
            }
            shared_heights[level] = height;
            previous = height;
        }
        target_status[0] = 2u;
        return;
    }
    if (column >= params.columns) { return; }
    status[column] = 1u;
    if (target_status[0] != 2u) { status[column] = 3u; return; }
    var previous = -1.0;
    var previous_target = -1.0;
    for (var level = 0u; level < params.levels; level++) {
        let lane = native[level*params.columns+column];
        let z = shared_heights[level];
        if (!finite(lane.x) || !finite(lane.y) || !finite(lane.z) || !finite(lane.w) ||
            lane.w <= 0.0 || lane.x <= previous || !finite(z) || z <= previous_target ||
            (level == 0u && (lane.x != 0.0 || z != 0.0))) { status[column] = 3u; return; }
        previous = lane.x;
        previous_target = z;
    }
    let top = native[(params.levels-1u)*params.columns+column];
    var upper = 1u;
    for (var level = 0u; level < params.levels; level++) {
        let index = level*params.columns+column;
        let z = shared_heights[level];
        var value = top.yz;
        if (level == 0u) { value = native[column].yz; }
        else if (level != params.levels-1u && z <= top.x) {
            while (upper < params.levels-1u && native[upper*params.columns+column].x < z) { upper++; }
            let lower_lane = native[(upper-1u)*params.columns+column];
            let upper_lane = native[upper*params.columns+column];
            let dz1 = z-lower_lane.x;
            let dz2 = upper_lane.x-z;
            value = (lower_lane.yz*dz2 + upper_lane.yz*dz1)/(dz1+dz2);
        }
        if (!finite(value.x) || !finite(value.y)) { status[column] = 4u; return; }
        output_u[index] = value.x;
        output_v[index] = value.y;
    }
    status[column] = 2u;
}
