// Publish the completed scientific timestep only when every required sample succeeded.
struct Params { capacity: u32, active_count: u32, nx: u32, ny: u32,
    dt: f32, x_scale: f32, y_scale: f32, top_m: f32 };
@group(0) @binding(0) var<storage, read> staged: array<u32>;
@group(0) @binding(1) var<storage, read> status: array<u32>;
@group(0) @binding(2) var<storage, read_write> particles: array<u32>;
@group(0) @binding(3) var<uniform> params: Params;
fn failed() -> bool {
    for (var stage = 0u; stage < 6u; stage++) {
        if (status[stage * (params.capacity + 1u)] != 0u) { return true; }
    }
    return false;
}
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let lane = gid.y * (nwg.x * 64u) + gid.x;
    if (lane >= params.capacity || failed() || status[6u * (params.capacity + 1u)] != 0u) { return; }
    // Raw words preserve inactive lanes and padding, including after compaction.
    for (var word = 0u; word < 24u; word++) { particles[lane * 24u + word] = staged[lane * 24u + word]; }
}
