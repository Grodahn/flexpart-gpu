// #171 minimal fixture mutation: assign sampled scalar only after batch-wide device approval.
struct Params { capacity: u32, active_count: u32, pad0: u32, pad1: u32 };
@group(0) @binding(0) var<storage, read> values: array<f32>;
@group(0) @binding(1) var<storage, read> status: array<u32>;
@group(0) @binding(2) var<storage, read_write> state: array<f32>;
@group(0) @binding(3) var<uniform> params: Params;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    // A separate preceding dispatch completed all status writes across workgroups.
    if (status[0] != 0u) { return; }
    let lane = gid.y * (nwg.x * 64u) + gid.x;
    if (lane >= params.active_count) { return; }
    if (status[lane + 1u] != 2u) { return; }
    state[lane] = values[lane];
}
