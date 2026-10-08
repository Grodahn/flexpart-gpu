// #171 particle storage adapter only. Preserve signed cells and AGL metres directly.
// The canonical query ABI does not depend on this 96-byte producer input ABI.
struct Particle {
    pos_x: f32, pos_y: f32, pos_z: f32, cell_x: i32, cell_y: i32, flags: u32,
    mass: array<f32, 4>, vel_u: f32, vel_v: f32, vel_w: f32,
    turb_u: f32, turb_v: f32, turb_w: f32, time: i32, timestep: i32,
    time_mem: i32, time_split: i32, release_point: i32, class_id: i32, cbt: i32, pad0: u32,
};
struct Query {
    cell_x: i32, cell_y: i32, fraction_x: f32, fraction_y: f32,
    height_agl_m: f32, is_active: u32, pad0: u32, pad1: u32,
};
struct Params { capacity: u32, active_count: u32, pad0: u32, pad1: u32 };
@group(0) @binding(0) var<storage, read> particles: array<Particle>;
@group(0) @binding(1) var<storage, read_write> queries: array<Query>;
@group(0) @binding(2) var<uniform> params: Params;
@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(num_workgroups) nwg: vec3<u32>) {
    let lane = gid.y * (nwg.x * 64u) + gid.x;
    if (lane >= params.capacity) { return; }
    queries[lane] = Query(0, 0, 0.0, 0.0, 0.0, 0u, 0u, 0u);
    if (lane >= params.active_count) { return; }
    let p = particles[lane];
    queries[lane] = Query(p.cell_x, p.cell_y, p.pos_x, p.pos_y, p.pos_z, p.flags & 1u, 0u, 0u);
}
