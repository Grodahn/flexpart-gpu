// #171 finite-result gate, separate from canonical interpolation equations.
struct Params { capacity: u32, active_count: u32, pad0: u32, pad1: u32 };
@group(0) @binding(0) var<storage, read> values: array<f32>;
@group(0) @binding(1) var<storage, read_write> status: array<atomic<u32>>;
@group(0) @binding(2) var<uniform> params: Params;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let lane = gid.y * (nwg.x * 64u) + gid.x;
    if (lane >= params.active_count) { return; }
    let reason = atomicLoad(&status[lane + 1u]);
    if (reason == 0u || reason > 2u) { return; }
    if (reason == 1u || (bitcast<u32>(values[lane]) & 0x7f800000u) == 0x7f800000u) {
        atomicStore(&status[lane + 1u], 9u);
        atomicOr(&status[0], 1u);
    }
}
