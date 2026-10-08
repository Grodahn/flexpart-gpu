// Fail-closed timestep eligibility after every Petterssen field and arithmetic stage.
struct Particle {
    pos_x: f32, pos_y: f32, pos_z: f32, cell_x: i32, cell_y: i32, flags: u32,
    mass: array<f32, 4>, vel_u: f32, vel_v: f32, vel_w: f32,
    turb_u: f32, turb_v: f32, turb_w: f32, time: i32, timestep: i32,
    time_mem: i32, time_split: i32, release_point: i32, class_id: i32, cbt: i32, pad0: u32,
};
struct Params { capacity: u32, active_count: u32, nx: u32, ny: u32,
    dt: f32, x_scale: f32, y_scale: f32, top_m: f32 };
@group(0) @binding(0) var<storage, read_write> staged: array<Particle>;
@group(0) @binding(1) var<storage, read> status: array<u32>;
@group(0) @binding(2) var<uniform> params: Params;
fn failed() -> bool {
    for (var stage = 0u; stage < 6u; stage++) {
        if (status[stage * (params.capacity + 1u)] != 0u) { return true; }
    }
    return false;
}
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let lane = gid.y * (nwg.x * 64u) + gid.x;
    if (lane >= params.capacity) { return; }
    // Inactive private lanes cannot feed downstream particle physics on fatal status.
    if (failed() || status[6u * (params.capacity + 1u)] != 0u) { staged[lane].flags = 0u; }
}
