// #171 device-side status reset. No physical calculation; each use starts fresh.
struct Params { capacity: u32, active_count: u32, pad0: u32, pad1: u32 };
@group(0) @binding(0) var<storage, read_write> status: array<atomic<u32>>;
@group(0) @binding(1) var<uniform> params: Params;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let lane = gid.y * (nwg.x * 64u) + gid.x;
    if (lane == 0u) { atomicStore(&status[0], 0u); }
    if (lane >= params.capacity) { return; }
    atomicStore(&status[lane + 1u], select(0u, 1u, lane < params.active_count));
}
